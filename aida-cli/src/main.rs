// Thin binary stub — the whole CLI lives in the library target so the
// entry-point boundary is clean and testable (ADR-16).
// trace:STORY-772 trace:ADR-16 | ai:claude
fn main() {
    // trace:TASK-3429 | ai:codex
    aida_cli_lib::command_registry::register_mcp_handler(aida_mcp::run_mcp_serve);
    aida_cli_lib::main_entry()
}
