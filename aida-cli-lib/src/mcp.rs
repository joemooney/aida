// CLI-local test harness for the extracted transport.
// trace:TASK-3429 | ai:codex
include!("../../aida-mcp/src/mcp.rs");
#[cfg(test)]
#[path = "tests/mcp_bug_2017_schema_tests.rs"]
mod bug_2017_schema_tests;
#[cfg(test)]
#[path = "store_mutation_mcp_tests.rs"]
mod store_mutation_mcp_tests;
#[cfg(test)]
#[path = "tests/mcp_tests.rs"]
mod tests;
