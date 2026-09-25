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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dash_values_are_refused_with_the_flag_named() {
        for v in ["--output=/tmp/x", "-o", "  --all", "-"] {
            let err = reject_option_like("--since", v).unwrap_err().to_string();
            assert!(err.contains("--since"), "{err}");
            assert!(err.contains("starts with `-`"), "{err}");
        }
        for v in ["v1.0", "HEAD~3", "main..HEAD", "a-b", "refs/tags/x"] {
            assert!(reject_option_like("--since", v).is_ok(), "{v}");
        }
    }
}
