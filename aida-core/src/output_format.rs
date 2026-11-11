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

#[cfg(test)]
mod tests {
    use super::{parse_output_format, OutputFormat};

    #[test]
    fn output_format_aliases_and_unknown_values() {
        for (raw, expected) in [
            ("human", OutputFormat::Human),
            (" TABLE ", OutputFormat::Human),
            ("toon", OutputFormat::Toon),
            (" AgEnT ", OutputFormat::Toon),
            (" JSON ", OutputFormat::Json),
        ] {
            assert_eq!(parse_output_format(Some(raw)), Some(expected), "{raw:?}");
        }
        for raw in [None, Some(""), Some("  "), Some("unsupported")] {
            assert_eq!(parse_output_format(raw), None, "{raw:?}");
        }
    }
}
