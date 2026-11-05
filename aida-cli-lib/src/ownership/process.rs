//! Holder validation using the shared PID/start identity probe.
// trace:BUG-2458 | ai:codex
use aida_core::process_probe::{Probe, ProbeSource, ProcessIdentity};

pub(super) fn invalid_holder(source: &ProbeSource, id: &ProcessIdentity) -> Option<String> {
    if id.pid <= 1 {
        return Some("holder identity invalid: init cannot own a lane".to_string());
    }
    let facts = source.facts(id.pid).ok()?;
    // Do not classify a recycled PID's new occupant as the recorded holder.
    if facts.identity != *id {
        return None;
    }
    let name = facts.command_label();
    if ["systemd", "init", "tini", "dumb-init"].contains(&name.as_str())
        || ["systemd", "init"].contains(&facts.name.as_str())
    {
        Some(format!(
            "holder identity invalid: {name} is an init/subreaper process"
        ))
    } else {
        None
    }
}

pub(super) fn probe(source: &ProbeSource, id: &ProcessIdentity) -> Probe {
    if let Some(why) = invalid_holder(source, id) {
        Probe::Unknown(why)
    } else {
        source.probe(id)
    }
}
