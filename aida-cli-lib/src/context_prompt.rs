use anyhow::Result;
use std::io::{BufRead, Write};

/// Deterministic help for consequential y/N prompts.
///
// trace:STORY-809 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextCard {
    pub(crate) decision: String,
    pub(crate) provenance: Vec<String>,
    pub(crate) answers: Vec<String>,
    pub(crate) recommended_default: String,
}

impl ContextCard {
    pub(crate) fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("\nContext\n");
        out.push_str(&format!("  Deciding: {}\n", self.decision));
        if !self.provenance.is_empty() {
            out.push_str("  Provenance:\n");
            for line in &self.provenance {
                out.push_str(&format!("    - {line}\n"));
            }
        }
        if !self.answers.is_empty() {
            out.push_str("  Answers:\n");
            for line in &self.answers {
                out.push_str(&format!("    - {line}\n"));
            }
        }
        out.push_str(&format!(
            "  Recommended default: {}\n",
            self.recommended_default
        ));
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextPromptAnswer {
    Yes,
    No,
    Help,
    Empty,
    Invalid,
}

fn parse_context_prompt_answer(raw: &str) -> ContextPromptAnswer {
    match raw.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => ContextPromptAnswer::Yes,
        "n" | "no" => ContextPromptAnswer::No,
        "?" => ContextPromptAnswer::Help,
        "" => ContextPromptAnswer::Empty,
        _ => ContextPromptAnswer::Invalid,
    }
}

fn prompt_suffix(default: bool) -> &'static str {
    if default {
        "[Y/n/?]"
    } else {
        "[y/N/?]"
    }
}

pub(crate) fn confirm_with_context(
    question: &str,
    default: bool,
    card: &ContextCard,
) -> Result<bool> {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    confirm_with_context_io(question, default, card, &mut input, &mut output)
}

fn confirm_with_context_io<R: BufRead, W: Write>(
    question: &str,
    default: bool,
    card: &ContextCard,
    input: &mut R,
    output: &mut W,
) -> Result<bool> {
    loop {
        write!(output, "{} {} ", question, prompt_suffix(default))?;
        output.flush()?;

        let mut answer = String::new();
        input.read_line(&mut answer)?;
        match parse_context_prompt_answer(&answer) {
            ContextPromptAnswer::Yes => return Ok(true),
            ContextPromptAnswer::No => return Ok(false),
            ContextPromptAnswer::Empty => return Ok(default),
            ContextPromptAnswer::Help => {
                writeln!(output, "{}", card.render())?;
            }
            ContextPromptAnswer::Invalid => {
                writeln!(output, "Please answer y, n, or ? for context.")?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn card() -> ContextCard {
        ContextCard {
            decision: "whether to open a PR before review".to_string(),
            provenance: vec!["branch bug-814: 1 commit; no open PR".to_string()],
            answers: vec![
                "y: prints the push/create command; reversible by closing the PR".to_string(),
                "n: leaves the spec held; reversible by rerunning review".to_string(),
            ],
            recommended_default: "n - avoid opening review surface until you inspect it"
                .to_string(),
        }
    }

    #[test]
    fn question_mark_prints_context_and_reasks() {
        let mut input = Cursor::new(b"?\ny\n".to_vec());
        let mut output = Vec::new();

        let accepted = confirm_with_context_io(
            "Open a PR from `bug-814` first?",
            false,
            &card(),
            &mut input,
            &mut output,
        )
        .unwrap();

        let rendered = String::from_utf8(output).unwrap();
        assert!(accepted);
        assert!(rendered.contains("Context"));
        assert!(rendered.contains("Deciding: whether to open a PR before review"));
        assert!(rendered.contains("branch bug-814: 1 commit; no open PR"));
        assert_eq!(
            rendered.matches("Open a PR from `bug-814` first?").count(),
            2
        );
    }

    #[test]
    fn empty_answer_uses_default() {
        let mut input = Cursor::new(b"\n".to_vec());
        let mut output = Vec::new();

        let accepted =
            confirm_with_context_io("Rebase PR-7 now?", true, &card(), &mut input, &mut output)
                .unwrap();

        assert!(accepted);
        assert!(String::from_utf8(output).unwrap().contains("[Y/n/?]"));
    }

    #[test]
    fn invalid_answer_reasks() {
        let mut input = Cursor::new(b"maybe\nn\n".to_vec());
        let mut output = Vec::new();

        let accepted = confirm_with_context_io(
            "Open a PR from `bug-814` first?",
            false,
            &card(),
            &mut input,
            &mut output,
        )
        .unwrap();

        let rendered = String::from_utf8(output).unwrap();
        assert!(!accepted);
        assert!(rendered.contains("Please answer y, n, or ? for context."));
        assert_eq!(
            rendered.matches("Open a PR from `bug-814` first?").count(),
            2
        );
    }
}
