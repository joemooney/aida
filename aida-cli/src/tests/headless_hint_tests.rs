use super::*;

/// Minimal POSIX-ish shell tokenizer for the round-trip test: splits
/// on unquoted whitespace, treats single-quoted spans as literal, and
/// `\` as a one-char escape — enough to reverse `shell_join_display`.
fn split_shell_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut it = s.chars();
    while let Some(c) = it.next() {
        match c {
            ' ' | '\t' => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                for q in it.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '\\' => {
                in_word = true;
                if let Some(esc) = it.next() {
                    cur.push(esc);
                }
            }
            other => {
                in_word = true;
                cur.push(other);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

#[test]
fn shell_join_display_quotes_only_what_needs_it() {
    // Bare words (flags, uuids, slugs) survive unquoted; an argument
    // with whitespace is single-quoted.
    let joined = shell_join_display(&[
        "-p".to_string(),
        "019e0000-0000-7000-8000-000000000000".to_string(),
        "/aida-review --pr 7".to_string(),
    ]);
    assert_eq!(
        joined,
        "-p 019e0000-0000-7000-8000-000000000000 '/aida-review --pr 7'"
    );
    // An embedded single quote round-trips through the '\'' escape.
    assert_eq!(shell_join_display(&["it's".to_string()]), "'it'\\''s'");
    // An empty element still produces a token, not a gap.
    assert_eq!(shell_join_display(&[String::new()]), "''");
}

/// BUG-225 acceptance: the `--no-launch --no-human` hint, copied and
/// pasted, must invoke exactly what `exec_claude_headless` runs.
/// `headless_launch_hint` is built from `claude_headless_args`, so the
/// printed string shell-splits back to `claude` + that exact argv.
#[test]
fn headless_launch_hint_round_trips_to_real_argv() {
    for prompt in [
        "/aida-review --pr 7",
        "/aida-pickup --auto-first",
        "it's a prompt",
    ] {
        let sid = "019e34ab-a8e0-7530-9569-e8ea6783af4a";
        let hint = headless_launch_hint(prompt, sid, false);
        let parsed = split_shell_words(&hint);

        // STORY-278: hint prefixes `AIDA_HEADLESS=1` to mirror the env
        // `exec_claude_headless` sets via `.env(...)`.
        let mut expected = vec!["AIDA_HEADLESS=1".to_string(), "claude".to_string()];
        expected.extend(session::claude_headless_args(prompt, sid));
        assert_eq!(parsed, expected, "hint did not round-trip: {hint}");

        // The two drift bugs BUG-225 names, asserted directly: the
        // hint carries `--session-id`, and the prompt is the LAST
        // argv element (a positional, mirroring the real launch).
        assert!(
            parsed.contains(&"--session-id".to_string()),
            "missing --session-id: {hint}"
        );
        assert_eq!(parsed.last().unwrap().as_str(), prompt);
    }
}

/// BUG-342: every unattended Claude launch must route through the shared
/// headless argv builders. Those builders carry
/// `--disallowed-tools AskUserQuestion`; direct `claude -p` construction
/// or plain `claude --resume` under no-human mode is the bypass class.
/// STORY-683 added the vendor-neutral `headless_vendor_args` builder (which
/// delegates to `claude_headless_args_with_posture` for the Claude arm and
/// builds `codex exec` for the Codex arm), so it counts as a shared builder
/// too. TASK-894 added `advisor_tier_program_and_args` — the advisor-tier
/// analogue that delegates to the same Claude builders (resume/cold-boot)
/// and to `codex_headless_args` — so it counts as a shared builder too.
/// BUG-705 added `compose_headless_command` — the single composition BOTH
/// the spawn and exec paths build from (it wraps `headless_vendor_args` +
/// the agent-program resolver + the OS wrapper) — so it counts too.
// trace:BUG-342 trace:STORY-683 trace:TASK-894 trace:BUG-705 | ai:codex
#[test]
fn headless_env_launches_route_through_shared_argv_builders() {
    fn assert_env_setter_has_builder_context(file: &str, src: &str) {
        let lines: Vec<&str> = src.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if !line.contains(".env(\"AIDA_HEADLESS\", \"1\")") {
                continue;
            }
            let start = idx.saturating_sub(12);
            let end = (idx + 3).min(lines.len());
            let window = lines[start..end].join("\n");
            assert!(
                    window.contains("claude_headless_args")
                        || window.contains("claude_headless_resume_args")
                        || window.contains("headless_vendor_args")
                        || window.contains("advisor_tier_program_and_args")
                        || window.contains("compose_headless_command"),
                    "{file}: AIDA_HEADLESS claude launch at line {} does not use a shared headless argv builder:\n{window}",
                    idx + 1
                );
        }
    }

    assert_env_setter_has_builder_context("session.rs", include_str!("../session.rs"));
    assert_env_setter_has_builder_context("main.rs", include_str!("../main.rs"));
}

/// BUG-342 regression for the actual bypass: `QueueWorkLaunch::Resume`
/// used to ignore `no_human` and call plain `claude --resume`, so the
/// BUG-327 builder-level AskUserQuestion denial never reached resumed
/// implementer/reviewer sessions.
// trace:BUG-342 | ai:codex
#[test]
fn no_human_resume_paths_use_headless_resume_launcher() {
    let src = include_str!("../main.rs").replace("\r\n", "\n");
    let count = src
        .matches("session::spawn_claude_headless_resume(")
        .count();
    assert!(
        count >= 3,
        "expected implementer resume, standalone reviewer resume, and advisor resume \
             to use the shared headless resume launcher; found {count}"
    );
    assert!(
        src.contains("if no_human {\n                let log_path = project_root"),
        "QueueWorkLaunch::Resume should branch on no_human before plain claude --resume"
    );
}
