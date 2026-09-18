use anyhow::{bail, Result};

use aida_core::{
    get_type_protocol, seed_missing_type_protocols, RequirementType, RequirementsStore, Storage,
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
        ProtocolCommand::Show { req_type } => {
            let Some(protocol) = get_type_protocol(&store, req_type) else {
                bail!("no type protocol found for `{req_type}`; run `aida protocol list`");
            };
            println!(
                "protocol: {} [{}]\n{}",
                protocol.req_type, protocol.meta_id, protocol.body
            );
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
