//! Spec-ID command arguments typed as a friendly node alias
//! (`TYPE-<LABEL>-NNN`) resolve to the canonical id before a command acts on
//! them, so leases, branch names and queue entries stay canonical.
// trace:BUG-B6B1278890774EF993D2EBC327862470-015 | ai:claude

/// Install the store's node-alias map once per process. The store-backed
/// dispatch installs it before running a command; early-dispatch commands
/// (`claim`, `worktree`, `session`, `pr`) run before that, so their argument
/// handling calls this first. Best-effort: no reachable store leaves the map
/// empty and every id is kept as typed. Unit tests install their own map
/// ([`aida_core::node_alias::install_for_thread`]); loading the map of the
/// repository the test binary happens to run in would leak into every test.
pub(crate) fn ensure_alias_map() {
    if cfg!(test) {
        return;
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if let Some(store) = aida_core::store_locate::detect_distributed_store() {
            aida_core::node_alias::install_from_store(&store);
        }
    });
}

/// The canonical id a spec-ID argument names, literal id first (see
/// [`aida_core::node_alias::expand_or_keep`]). Anything that is not a bound
/// alias form — a canonical id, a UUID, `PR-14`, `batch:NAME`, an unknown
/// label — comes back unchanged, so a not-found still names what was typed.
pub(crate) fn canonical(id: &str) -> String {
    ensure_alias_map();
    aida_core::node_alias::expand_or_keep(id)
}

/// [`canonical`] in place.
pub(crate) fn canonicalize(slot: &mut String) {
    *slot = canonical(slot);
}

/// [`canonical`] in place, for an optional argument.
pub(crate) fn canonicalize_opt(slot: &mut Option<String>) {
    if let Some(id) = slot.as_mut() {
        canonicalize(id);
    }
}

/// [`canonical`] applied to each entry of a comma-separated list argument.
pub(crate) fn canonicalize_csv(slot: &mut String) {
    *slot = slot
        .split(',')
        .map(|part| {
            let trimmed = part.trim();
            match canonical(trimmed) {
                same if same == trimmed => part.to_string(),
                expanded => expanded,
            }
        })
        .collect::<Vec<_>>()
        .join(",");
}

/// clap `value_parser` for a spec-ID argument: the handler only ever sees the
/// [`canonical`] id, so keys it writes (verdicts, ledgers, leases) and string
/// comparisons it makes stay canonical. Never fails.
pub(crate) fn spec_id(raw: &str) -> Result<String, String> {
    Ok(canonical(raw))
}

/// [`spec_id`] for a comma-separated list argument.
pub(crate) fn spec_id_csv(raw: &str) -> Result<String, String> {
    let mut list = raw.to_string();
    canonicalize_csv(&mut list);
    Ok(list)
}

/// [`canonicalize_csv`] for an optional argument.
pub(crate) fn canonicalize_csv_opt(slot: &mut Option<String>) {
    if let Some(list) = slot.as_mut() {
        canonicalize_csv(list);
    }
}

#[cfg(test)]
#[path = "tests/bug_015_spec_arg_tests.rs"]
mod tests;
