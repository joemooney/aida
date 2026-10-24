//! STORY-1655: audited solo-human self-approval.
//!
//! A one-person project has no independent session to approve from, so the
//! BUG-1918 author gate would block every approval. `[review]
//! solo_human_self_approval = true` lets a human who passed the human-at-TTY
//! floor approve work their own session claimed, under three binding limits
//! from the operator decision on the spec:
//!
//! - S1: the key is read ONLY from the user's `~/.aida/config.toml`. A value in
//!   a project's `.aida/config.toml` (or its trusted default-branch copy) is
//!   ignored with a warning, so an agent's PR can never turn the mode on.
//! - S2: the TTY floor alone is not a human floor (a managed agent at a TTY
//!   passes it), so a solo approval also needs a typed challenge (the spec id
//!   and PR number) read from the controlling terminal, never from stdin. A
//!   piped or scripted answer is refused.
//! - S3: the AIDA repository itself never runs solo mode.
//!
//! This is a separate durable key, not the TTL presence `aida solo` sets.
// trace:STORY-1655 | ai:claude
// mandate-gate

use colored::Colorize;
use std::io::{BufRead, Write};
use std::path::Path;

/// The `[review]` key that turns the mode on.
pub(crate) const SOLO_KEY: &str = "solo_human_self_approval";

/// What `aida doctor` and `aida awaiting` print while the mode is on.
pub(crate) const MODE_ON_LINE: &str =
    "review: solo-human mode ON (self-approval permitted for TTY humans)";

/// The warning printed when a project config names the key.
pub(crate) const PROJECT_VALUE_IGNORED: &str =
    "[review] solo_human_self_approval in the project's .aida/config.toml is ignored; \
     solo-human self-approval can only be turned on in ~/.aida/config.toml";

/// Pure parse of `[review] solo_human_self_approval` from the GLOBAL config.
/// `Ok(false)` when unset; `Err` (treated as off) when it is not a boolean.
pub(crate) fn global_value_from_toml(value: &toml::Value) -> Result<bool, String> {
    match value.get("review").and_then(|r| r.get(SOLO_KEY)) {
        None => Ok(false),
        Some(v) => v
            .as_bool()
            .ok_or_else(|| format!("[review] {SOLO_KEY} must be a boolean (true/false)")),
    }
}

/// The global key as written in `~/.aida/config.toml`. Every failure (no
/// home, unreadable, malformed, wrong type) resolves to off; `Err` names why
/// for `aida config show`.
pub(crate) fn global_value_quiet() -> Result<bool, String> {
    let Some(home) = crate::home_dir() else {
        return Err("home directory could not be resolved".to_string());
    };
    let body = match std::fs::read_to_string(home.join(".aida").join("config.toml")) {
        Ok(body) => body,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("could not read ~/.aida/config.toml: {e}")),
    };
    let value = toml::from_str::<toml::Value>(&body)
        .map_err(|e| format!("~/.aida/config.toml did not parse: {e}"))?;
    global_value_from_toml(&value)
}

/// Does a project config body name the key? Display-only: the result feeds a
/// warning and NEVER the mode itself.
pub(crate) fn project_body_names_key(body: &str) -> bool {
    toml::from_str::<toml::Value>(body)
        .ok()
        .is_some_and(|v| v.get("review").and_then(|r| r.get(SOLO_KEY)).is_some())
}

/// Does the branch-local `.aida/config.toml` or its trusted default-branch
/// copy name the key?
pub(crate) fn project_value_present(project_root: &Path) -> bool {
    std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
        .ok()
        .is_some_and(|body| project_body_names_key(&body))
        || crate::trusted_config::read_trusted_config_toml_local(project_root)
            .is_some_and(|body| project_body_names_key(&body))
}

/// Print [`PROJECT_VALUE_IGNORED`] on stderr when a project config names the
/// key; returns whether it did.
pub(crate) fn warn_if_project_value(project_root: &Path) -> bool {
    let present = project_value_present(project_root);
    if present {
        eprintln!(
            "  {} {PROJECT_VALUE_IGNORED}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
    }
    present
}

/// The mode as it applies to one project: the global key is on and the
/// project is not the AIDA repository (S3).
pub(crate) fn effective_with(global_on: bool, is_aida_repo: bool) -> bool {
    global_on && !is_aida_repo
}

/// Is solo-human self-approval in force for `project_root`? Reads the global
/// key only; a project value is warned about and ignored.
pub(crate) fn effective(project_root: &Path) -> bool {
    warn_if_project_value(project_root);
    effective_with(
        global_value_quiet().unwrap_or(false),
        crate::is_aida_repo(project_root),
    )
}

/// What the human must type to confirm a solo self-approval.
pub(crate) fn expected_answer(spec: &str, pr: u64) -> String {
    format!("{} {pr}", spec.trim().to_ascii_uppercase())
}

/// Case- and whitespace-insensitive match; a `#` before the PR number is
/// accepted.
pub(crate) fn answer_matches(typed: &str, expected: &str) -> bool {
    let norm = |s: &str| {
        s.split_whitespace()
            .map(|w| w.trim_start_matches('#').to_ascii_uppercase())
            .collect::<Vec<_>>()
    };
    let typed = norm(typed);
    !typed.is_empty() && typed == norm(expected)
}

/// The challenge with its inputs injected. `stdin_is_terminal` false means
/// the command was scripted or piped: refused before anything is read, so a
/// piped answer never counts. `tty` is the controlling terminal (prompt
/// written to it, answer read from it); `None` refuses.
pub(crate) fn run_challenge_with<R: BufRead, W: Write>(
    stdin_is_terminal: bool,
    tty: Option<(R, W)>,
    spec: &str,
    pr: u64,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        stdin_is_terminal,
        "refused: solo-human self-approval needs a person typing at a terminal; this command's \
         input is piped or scripted"
    );
    let Some((mut input, mut output)) = tty else {
        anyhow::bail!(
            "refused: solo-human self-approval needs a controlling terminal to confirm on, and \
             none could be opened"
        );
    };
    let expected = expected_answer(spec, pr);
    write!(
        output,
        "Solo-human self-approval: you are approving work your own session claimed.\n\
         Type the spec id and PR number to confirm ({expected}): "
    )?;
    output.flush()?;
    let mut typed = String::new();
    input.read_line(&mut typed)?;
    anyhow::ensure!(
        answer_matches(&typed, &expected),
        "refused: the solo-human confirmation did not match `{expected}`"
    );
    Ok(())
}

type ControllingTty = (std::io::BufReader<std::fs::File>, std::fs::File);

/// The controlling terminal, opened read/write. Never opened under test.
#[cfg(all(unix, not(test)))]
fn open_controlling_tty() -> Option<ControllingTty> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    let reader = file.try_clone().ok()?;
    Some((std::io::BufReader::new(reader), file))
}

#[cfg(any(not(unix), test))]
fn open_controlling_tty() -> Option<ControllingTty> {
    None
}

#[cfg(test)]
thread_local! {
    static TEST_TERMINAL_ANSWER: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` as a human typing `answer` at a terminal: the record path's
/// human-at-TTY floor passes and the challenge reads `answer` (tests only).
// trace:TASK-1955 | ai:claude (TASK-1808)
#[cfg(test)]
pub(crate) fn with_test_human_terminal<R>(answer: &str, f: impl FnOnce() -> R) -> R {
    struct Reset(Option<String>);
    impl Drop for Reset {
        fn drop(&mut self) {
            let prev = self.0.take();
            TEST_TERMINAL_ANSWER.with(|a| *a.borrow_mut() = prev);
        }
    }
    let prev = TEST_TERMINAL_ANSWER.with(|a| a.borrow_mut().replace(answer.to_string()));
    let _reset = Reset(prev);
    f()
}

/// Is a [`with_test_human_terminal`] human at the terminal? (tests only)
#[cfg(test)]
pub(crate) fn test_human_at_terminal() -> bool {
    TEST_TERMINAL_ANSWER.with(|a| a.borrow().is_some())
}

/// Run the typed challenge for a solo self-approval of `spec` at PR `pr`,
/// reading the answer from the controlling terminal, never from stdin.
pub(crate) fn run_challenge(spec: &str, pr: u64) -> anyhow::Result<()> {
    #[cfg(test)]
    if let Some(answer) = TEST_TERMINAL_ANSWER.with(|a| a.borrow().clone()) {
        let tty = (std::io::Cursor::new(answer.into_bytes()), Vec::new());
        return run_challenge_with(true, Some(tty), spec, pr);
    }
    let stdin_is_terminal = crate::authority_stdin_is_terminal();
    let tty = if stdin_is_terminal {
        open_controlling_tty()
    } else {
        None
    };
    run_challenge_with(stdin_is_terminal, tty, spec, pr)
}

#[cfg(test)]
#[path = "tests/story_1655_solo_review_tests.rs"]
mod story_1655_solo_review_tests;
