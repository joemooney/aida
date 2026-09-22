use anyhow::{bail, Result};

use aida_core::{
    get_type_protocol, resolve_protocol, seed_missing_type_protocols, CachedGitBackend,
    DatabaseBackend, ListFilter, Requirement, RequirementType, RequirementsStore, Storage,
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

pub(crate) fn protocol_for_requirement(
    store: &RequirementsStore,
    req_type: &RequirementType,
) -> Option<aida_core::TypeProtocol> {
    get_type_protocol(store, &req_type.to_string())
}

/// Render the protocol block used by interactive pickup surfaces.
// trace:TASK-1283 | ai:codex
pub(crate) fn pickup_block_for_requirement(
    store: &RequirementsStore,
    req: &Requirement,
) -> Option<String> {
    protocol_for_requirement(store, &req.req_type).map(|protocol| protocol.pickup_block())
}

/// Resolve the compact reminder for the spec held by the current worktree.
/// Lease discovery stays with the CLI layer; keeping this lookup pure makes the
/// quiet/no-lease contract directly regression-testable.
// trace:TASK-1283 | ai:codex
#[cfg(test)]
pub(crate) fn notice_line_for_scope(
    store: &RequirementsStore,
    leased_scope: Option<&str>,
) -> Option<String> {
    let scope = leased_scope?;
    let req = store.requirements.iter().find(|req| {
        req.display_id().eq_ignore_ascii_case(scope)
            || req
                .spec_id
                .as_deref()
                .is_some_and(|id| id.eq_ignore_ascii_case(scope))
    })?;
    protocol_for_requirement(store, &req.req_type).map(|protocol| protocol.notice_line())
}

/// Resolve the per-turn reminder without loading the requirement store. The
/// lease names one canonical spec, and the cache identifies the one editable
/// META record for its type; both authoritative records are then read directly
/// from their YAML objects.
// trace:BUG-1569 | ai:codex
pub(crate) fn targeted_notice_line_for_scope(
    backend: &CachedGitBackend,
    leased_scope: &str,
) -> Result<Option<String>> {
    let Some(req) = backend.get_requirement_by_spec_id(leased_scope)? else {
        return Ok(None);
    };
    let protocol_tag = format!("protocol:{}", req.req_type.to_string().to_ascii_lowercase());
    let filter = ListFilter {
        tags: vec![protocol_tag.clone()],
        ..ListFilter::default()
    };
    let summary = backend
        .cache()
        .list_summaries(&filter)?
        .into_iter()
        .find(|candidate| candidate.req_type.eq_ignore_ascii_case("meta"));
    let Some(summary) = summary else {
        if backend.cache_snapshot_is_stale()? {
            anyhow::bail!(
                "stale cache snapshot has no {protocol_tag} record; refusing to claim no protocol"
            );
        }
        return Ok(None);
    };
    let Some(protocol) = backend.get_requirement(&summary.id)? else {
        anyhow::bail!(
            "cached protocol {} no longer resolves to an authoritative requirement",
            summary.spec_id.as_deref().unwrap_or("<unknown>")
        );
    };
    if protocol.req_type != RequirementType::Meta
        || !protocol
            .tags
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case(&protocol_tag))
    {
        anyhow::bail!("cached protocol record failed authoritative tag/type validation");
    }
    let mut protocol_only = RequirementsStore::default();
    protocol_only.requirements.push(protocol);
    Ok(protocol_for_requirement(&protocol_only, &req.req_type).map(|value| value.notice_line()))
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

/// Prefix a queue-work/headless prompt with its resolved type+lane contract.
// trace:TASK-1283 | ai:codex
pub(crate) fn prepend_resolved_pickup_protocol(
    store: &RequirementsStore,
    req: &Requirement,
    prompt: String,
) -> String {
    resolved_protocol_for_requirement(store, req)
        .map(|protocol| format!("{}\n\n{prompt}", protocol.pickup_block()))
        .unwrap_or(prompt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::{seed_meta_requirements, PROTOCOL_PICKUP_LINE_CAP};

    fn protocol_store() -> RequirementsStore {
        let mut store = RequirementsStore::default();
        seed_meta_requirements(&mut store).unwrap();
        store
    }

    fn requirement(kind: RequirementType, id: &str) -> Requirement {
        let mut req = Requirement::new(
            format!("{id} fixture"),
            "## Acceptance\n- fixture acceptance".to_string(),
        );
        req.req_type = kind;
        req.spec_id = Some(id.to_string());
        req.agreed_id = Some(id.to_string());
        req
    }

    fn assert_pickup_contract(block: &str, acceptance: &str) {
        let combined = format!("{block}\n\n{acceptance}");
        assert!(combined.starts_with("##"));
        assert!(block.contains("META-"), "protocol must cite its META row");
        assert!(block.contains("spec acceptance"));
        assert!(
            combined.find("spec acceptance").unwrap() < combined.rfind("## Acceptance").unwrap()
        );
        let body_lines = block.lines().count().saturating_sub(2);
        assert!(body_lines <= PROTOCOL_PICKUP_LINE_CAP);
    }

    #[test]
    fn protocol_queue_work_spike_pickup_is_resolved_cited_capped_and_precedes_acceptance() {
        let store = protocol_store();
        let req = requirement(RequirementType::Spike, "SPIKE-9001");
        let prompt = prepend_resolved_pickup_protocol(&store, &req, req.description.clone());
        assert!(prompt.contains("lane:research"));
        let block = prompt.split("\n\n## Acceptance").next().unwrap();
        assert_pickup_contract(block, &req.description);
    }

    #[test]
    fn protocol_do_bug_pickup_is_cited_capped_and_precedes_acceptance() {
        let store = protocol_store();
        let req = requirement(RequirementType::Bug, "BUG-9001");
        let block = pickup_block_for_requirement(&store, &req).unwrap();
        assert!(block.contains("Type protocol: bug"));
        assert_pickup_contract(&block, &req.description);
    }

    #[test]
    fn protocol_worktree_enter_story_pickup_is_cited_capped_and_precedes_acceptance() {
        let store = protocol_store();
        let req = requirement(RequirementType::Story, "STORY-9001");
        let block = pickup_block_for_requirement(&store, &req).unwrap();
        assert!(block.contains("Type protocol: story"));
        assert_pickup_contract(&block, &req.description);
    }

    #[test]
    fn protocol_pickup_with_unseeded_type_injects_nothing() {
        let store = RequirementsStore::default();
        let req = requirement(RequirementType::Functional, "FR-9001");
        assert!(pickup_block_for_requirement(&store, &req).is_none());
        assert!(resolved_protocol_for_requirement(&store, &req).is_none());
        assert_eq!(
            prepend_resolved_pickup_protocol(&store, &req, req.description.clone()),
            req.description
        );
    }

    #[test]
    fn protocol_awaiting_notice_is_quiet_without_a_lease() {
        let store = protocol_store();
        assert!(notice_line_for_scope(&store, None).is_none());
    }

    #[test]
    fn protocol_awaiting_notice_exists_only_for_the_leased_spec_lifecycle() {
        let mut store = protocol_store();
        let req = requirement(RequirementType::Bug, "BUG-9002");
        store.requirements.push(req);
        let held = notice_line_for_scope(&store, Some("BUG-9002")).unwrap();
        assert!(held.starts_with("protocol: bug [META-"));
        // `queue done` releases the lease, so the awaiting path supplies no scope.
        assert!(notice_line_for_scope(&store, None).is_none());
    }

    #[test]
    fn protocol_seed_cli_path_adds_four_missing_types_and_is_idempotent() {
        let mut store = protocol_store();
        store.requirements.retain(|req| {
            ![
                "protocol:story",
                "protocol:task",
                "protocol:decision",
                "protocol:doc",
            ]
            .iter()
            .any(|tag| req.tags.contains(*tag))
        });
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().join("requirements.yaml"));
        storage.save(&store).unwrap();

        assert_eq!(seed_missing_protocols(&storage).unwrap(), 4);
        assert_eq!(seed_missing_protocols(&storage).unwrap(), 0);
        let store = storage.load().unwrap();
        for kind in TYPES {
            assert!(get_type_protocol(&store, kind).is_some(), "missing {kind}");
        }
    }
}
