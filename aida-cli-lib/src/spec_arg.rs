// trace:TASK-3783 | ai:codex
// Keep cfg(test) in the consumer: cmdkit is a non-test dependency of this
// crate's unit-test binary, even when this module is compiled for tests.
pub(crate) fn ensure_alias_map() {
    #[cfg(not(test))]
    aida_cmdkit::spec_arg::ensure_alias_map();
}

pub(crate) fn canonical(id: &str) -> String {
    aida_cmdkit::spec_arg::canonical_with(id, ensure_alias_map)
}

pub(crate) fn spec_id(raw: &str) -> Result<String, String> {
    Ok(canonical(raw))
}

pub(crate) fn spec_id_csv(raw: &str) -> Result<String, String> {
    aida_cmdkit::spec_arg::spec_id_csv_with(raw, ensure_alias_map)
}

#[cfg(test)]
#[path = "tests/bug_015_spec_arg_tests.rs"]
mod tests;
