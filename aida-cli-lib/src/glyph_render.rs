// trace:TASK-3428 | ai:codex

/// Render a registry [`crate::glyphs::Glyph`] honoring the active glyph profile
/// (`AIDA_GLYPHS` env / `[ui] glyphs` config / default unicode). The crate-root
/// twin of the module-local `glyph()` helpers in `auto_complete.rs`,
/// `status_cleanup.rs`, etc. Called fully-qualified (`crate::glyph(...)`) from
/// the migrated literal sites so it resolves uniformly from main-code and
/// inline test-module scopes alike. Default (unicode) output is byte-for-byte
/// identical to the raw literals it replaced.
// trace:TASK-840 | ai:claude
// trace:TASK-3429 | ai:codex
pub(crate) fn glyph(g: crate::glyphs::Glyph) -> &'static str {
    crate::glyphs::get(g, crate::find_project_root().ok().as_deref())
}
