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
// trace:BUG-1622 | ai:claude

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

#[cfg(test)]
mod tests {
    use super::*;

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
