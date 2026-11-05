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
    // Linux PR_GET_CHILD_SUBREAPER queries only the calling process. There is
    // no remote procfs flag: these are known init executables, not a generic
    // kernel subreaper test. Never claim that an arbitrary subreaper is covered.
    let name = facts.command_label();
    if ["systemd", "init", "tini", "dumb-init"].contains(&name.as_str())
        || ["systemd", "init"].contains(&facts.name.as_str())
    {
        Some(format!(
            "holder identity invalid: {name} is a known init/subreaper executable"
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
