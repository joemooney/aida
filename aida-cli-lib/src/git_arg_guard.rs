//! Keeps a user-supplied value from being read by git as an option.
//!
//! A value that becomes a git revision, ref, range or path argument goes
//! through two independent guards:
//!
//! 1. [`reject_option_like`] refuses any value that starts with `-`, with an
//!    error naming the flag it came from. No ref, tag, range or path this CLI
//!    accepts legitimately starts with a dash.
//! 2. The git call puts [`END_OF_OPTIONS`] before revision arguments (and
//!    `--` before paths), so even a value that slipped past the check is
//!    parsed as a revision, never as an option such as `--output=<path>`.
//!
//! A branch name that must land in a SHELL string (rather than an argv) goes
//! through [`is_shell_safe_branch_name`] instead.
// trace:BUG-1622 | ai:claude
// trace:BUG-1624 | ai:claude

use anyhow::{bail, Result};

/// Marks the end of git's options: every later argument is a revision or a
/// path, even when it starts with `-`. Supported since git 2.24.
pub(crate) const END_OF_OPTIONS: &str = "--end-of-options";

/// Refuse a user-supplied git ref/revision/path that starts with `-`, which
/// git would otherwise read as an option. `flag` names where the value came
/// from (e.g. `--since`) so the error points at it.
pub(crate) fn reject_option_like(flag: &str, value: &str) -> Result<()> {
    if is_option_like(value) {
        bail!(
            "invalid {flag} value `{}`: it starts with `-`, so git would read it as an \
             option; give a git ref, tag or revision instead",
            value.trim()
        );
    }
    Ok(())
}

/// True when `value` (ignoring leading whitespace) starts with `-`.
pub(crate) fn is_option_like(value: &str) -> bool {
    value.trim_start().starts_with('-')
}

/// True when `value` is an abbreviated or full commit ID: 7 to 64 hex
/// digits (SHA-1 or SHA-256). A value read from the store, a commit trailer
/// or a verdict must pass this before it becomes a git revision argument.
pub(crate) fn is_hex_sha(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// True when `name` is a branch name that may be spliced, unquoted, into a
/// shell command line: it passes `git check-ref-format --branch` (run as an
/// argv, never through a shell) AND every character is shell-inert
/// (`[A-Za-z0-9._/+@-]`, not starting with `-`).
///
/// git's ref-name rules alone are not enough: they allow `;`, `$`, `(`,
/// backticks, quotes, `&`, `|` and `>`, any of which a shell would act on.
/// The character allowlist is what makes the value safe in every quoting
/// context (bare, inside `'…'`, inside `"…"`); the git check keeps the value
/// a real branch name. A forge or remote-derived default branch must pass
/// this before it reaches a shell string.
// trace:BUG-1624 | ai:claude
pub(crate) fn is_shell_safe_branch_name(name: &str) -> bool {
    if name.is_empty()
        || is_option_like(name)
        || !name.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'+' | b'@' | b'-')
        })
    {
        return false;
    }
    std::process::Command::new("git")
        .args(["check-ref-format", "--branch", name])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    // trace:BUG-1624 | ai:claude
    #[test]
    fn bug_1624_shell_safe_branch_names() {
        for ok in [
            "main",
            "master",
            "trunk",
            "release/1.2",
            "feat-x_y+z",
            "dev@2",
        ] {
            assert!(is_shell_safe_branch_name(ok), "{ok}");
        }
        for bad in [
            "",
            "main;touch pwned",
            "main$(touch pwned)",
            "main`touch pwned`",
            "x';touch pwned;'",
            "a\"b",
            "a|b",
            "a&b",
            "a>b",
            "a b",
            "-main",
            "--upload-pack=x",
            "main..x",
            "main.lock",
            "@{-1}",
            "/main",
            "ma\nin",
        ] {
            assert!(!is_shell_safe_branch_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn dash_values_are_refused_with_the_flag_named() {
        for v in ["--output=injected.x", "-o", "  --all", "-"] {
            let err = reject_option_like("--since", v).unwrap_err().to_string();
            assert!(err.contains("--since"), "{err}");
            assert!(err.contains("starts with `-`"), "{err}");
        }
        for v in ["v1.0", "HEAD~3", "main..HEAD", "a-b", "refs/tags/x"] {
            assert!(reject_option_like("--since", v).is_ok(), "{v}");
        }
    }

    #[test]
    fn hex_sha_accepts_only_commit_ids() {
        for v in [
            "abcdef1",
            "0123456789abcdef0123456789abcdef01234567",
            &"a".repeat(64),
        ] {
            assert!(is_hex_sha(v), "{v}");
        }
        for v in [
            "",
            "abc123",
            "--output=x",
            "HEAD",
            "abcdefg",
            &"a".repeat(65),
            " abcdef1",
        ] {
            assert!(!is_hex_sha(v), "{v}");
        }
    }
}
