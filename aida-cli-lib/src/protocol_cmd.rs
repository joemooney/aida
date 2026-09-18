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

pub(crate) fn protocol_for_requirement(
    store: &RequirementsStore,
    req_type: &RequirementType,
) -> Option<aida_core::TypeProtocol> {
    get_type_protocol(store, &req_type.to_string())
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
