// trace:TASK-3783 | ai:codex
pub use aida_term::cache_output::*;

pub(crate) fn finish(
    scope: &aida_core::db::cache_refresh::CacheReadScope,
    success: bool,
    argv: &[String],
) {
    finish_with_format(
        scope,
        success,
        argv,
        crate::output_format_is_json(),
        crate::agent_output_mode,
    );
}
