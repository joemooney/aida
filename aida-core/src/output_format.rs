//! Shared output-format spellings for CLI and porcelain entrypoints.
// trace:STORY-2704 | ai:codex

/// An explicit output-format selection, independent of transport or terminal state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Human,
    Toon,
    Json,
}

/// Parse an environment format pin, accepting aliases and ignoring unknown values.
pub fn parse_output_format(raw: Option<&str>) -> Option<OutputFormat> {
    match raw?.trim().to_ascii_lowercase().as_str() {
        "human" | "table" => Some(OutputFormat::Human),
        "toon" | "agent" => Some(OutputFormat::Toon),
        "json" => Some(OutputFormat::Json),
        _ => None,
    }
}
