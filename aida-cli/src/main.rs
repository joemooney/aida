// Thin binary stub — the whole CLI lives in the library target so the
// entry-point boundary is clean and testable (ADR-16).
// trace:STORY-772 trace:ADR-16 | ai:claude
fn main() {
    aida_cli_lib::main_entry()
}
