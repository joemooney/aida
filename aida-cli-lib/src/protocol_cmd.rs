use anyhow::{bail, Result};

use aida_core::{get_type_protocol, RequirementType, RequirementsStore};

use crate::cli::ProtocolCommand;

const TYPES: &[&str] = &["spike", "bug", "story", "task", "decision", "doc"];

// trace:STORY-1221 | ai:codex
pub(crate) fn handle_protocol_command(
    cmd: &ProtocolCommand,
    store: &RequirementsStore,
) -> Result<()> {
    match cmd {
        ProtocolCommand::List => {
            for kind in TYPES {
                if let Some(protocol) = get_type_protocol(store, kind) {
                    println!("{:<10} {}", kind, protocol.meta_id);
                }
            }
        }
        ProtocolCommand::Show { req_type } => {
            let Some(protocol) = get_type_protocol(store, req_type) else {
                bail!("no type protocol found for `{req_type}`; run `aida protocol list`");
            };
            println!(
                "protocol: {} [{}]\n{}",
                protocol.req_type, protocol.meta_id, protocol.body
            );
        }
    }
    Ok(())
}

pub(crate) fn protocol_for_requirement(
    store: &RequirementsStore,
    req_type: &RequirementType,
) -> Option<aida_core::TypeProtocol> {
    get_type_protocol(store, &req_type.to_string())
}
