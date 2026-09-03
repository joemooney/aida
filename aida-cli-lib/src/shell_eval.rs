//! The dedicated channel for shell code the `aida()` wrapper evals.
//!
//! # The problem
//!
//! A handful of subcommands mutate the CALLING shell — `role enter` / `role
//! end`, `dev activate` / `dev deactivate`, `worktree enter` / `worktree exit`,
//! `session start` / `session end`. A subprocess cannot do that, so they print
//! shell code and the `aida()` wrapper function eval's it.
//!
//! Historically the contract was "stdout IS the shell code": the wrapper eval'd
//! the whole captured stdout. That made ordinary human-facing stdout an eval
//! candidate — when one of these commands failed, its error text was eval'd and
//! the operator saw a burst of `command not found` instead of the real message
//! (BUG-779). The first fix was exit-code discipline: eval only on exit 0. That
//! closes the observed symptom but leaves the *shape* wrong — any success path
//! that prints prose to stdout still leaks it into the eval.
//!
//! # The fix
//!
//! Shell code gets its own channel: a marker-delimited block inside stdout.
//!
//! ```text
//! Entered worktree for TASK-1171          <- ordinary stdout, DISPLAYED
//! #aida:eval:begin
//! cd '/home/joe/ai/aida-task-1171'        <- the payload, EVAL'D
//! export CARGO_TARGET_DIR='…'
//! #aida:eval:end
//! ```
//!
//! The wrapper slices the block out, evals ONLY that, and prints everything
//! else. Human output can never be eval'd again, no matter which path a
//! subcommand takes.
//!
//! # Version skew (the reason this is negotiated, not unconditional)
//!
//! An operator upgrading the binary still has the OLD wrapper sourced in the
//! shell they are standing in — it evals whole stdout and knows nothing about
//! markers. Emitting a marked block unconditionally would, at best, feed it two
//! extra lines; at worst mean a new verb's prose reaches its eval.
//!
//! So the block is **negotiated**, using the capability list the wrapper
//! already exports as `AIDA_SHELL_WRAPPER`:
//!
//! | wrapper | binary | behaviour |
//! |---------|--------|-----------|
//! | old (no `eval-block`) | new | bare payload — byte-identical to today |
//! | new (`eval-block`)    | old | no markers found → wrapper evals whole stdout (legacy path) |
//! | new                   | new | marked block: payload eval'd, prose displayed |
//! | none (raw `eval "$(aida …)"`) | new | bare payload — unchanged |
//!
//! Both markers are shell COMMENTS (`#…`), so even a naive whole-stdout eval of
//! a marked payload is inert rather than an error.
// trace:TASK-1171 | ai:claude

use std::sync::atomic::{AtomicUsize, Ordering};

/// Opening marker of the eval block. A shell comment, so a legacy
/// whole-stdout eval treats it as a no-op line.
pub(crate) const EVAL_BEGIN: &str = "#aida:eval:begin";

/// Closing marker of the eval block.
pub(crate) const EVAL_END: &str = "#aida:eval:end";

/// The capability token a wrapper advertises in `AIDA_SHELL_WRAPPER` when it
/// knows how to slice the marked block out of stdout.
pub(crate) const EVAL_BLOCK_CAP: &str = "eval-block";

/// The capability token a wrapper advertises when its `init` case branch can
/// consume a marked `cd` block from `aida init <DIR>` (STORY-780). Distinct
/// from `eval-block`: a wrapper may speak the channel for the legacy verbs
/// yet not route `init` through it at all — emitting markers at such a
/// wrapper would print them as inert comment noise.
// trace:STORY-780 | ai:claude
pub(crate) const INIT_CD_CAP: &str = "init-cd";

/// Nesting depth, so an outer emitter that internally calls another one still
/// produces exactly ONE block (a nested pair of markers would end the block
/// early and leak the tail into ordinary stdout).
static DEPTH: AtomicUsize = AtomicUsize::new(0);

/// Whole-token, case-insensitive probe over the comma-separated
/// `AIDA_SHELL_WRAPPER` capability marker. Pure, so the parsing is testable
/// without mutating the process-global env.
// trace:BUG-654 trace:TASK-1160 trace:TASK-1171 | ai:claude
pub(crate) fn marker_has_cap(marker: Option<&str>, cap: &str) -> bool {
    marker
        .map(|caps| caps.split(',').any(|c| c.trim().eq_ignore_ascii_case(cap)))
        .unwrap_or(false)
}

/// Pure decision half of [`wrapper_speaks_eval_block`]: given the caller's
/// `AIDA_SHELL_WRAPPER` value (`None` when no wrapper is installed), does that
/// wrapper know how to read a marked eval block?
// trace:TASK-1171 | ai:claude
pub(crate) fn marker_speaks_eval_block(marker: Option<&str>) -> bool {
    marker_has_cap(marker, EVAL_BLOCK_CAP)
}

/// Does the wrapper in the CALLER's shell speak the dedicated-channel protocol?
/// `false` for a stale wrapper and for no wrapper at all — both get the bare
/// (unmarked) payload, which is exactly what they have always consumed.
// trace:TASK-1171 | ai:claude
pub(crate) fn wrapper_speaks_eval_block() -> bool {
    marker_speaks_eval_block(std::env::var("AIDA_SHELL_WRAPPER").ok().as_deref())
}

/// Scope guard around a region of stdout that is SHELL CODE, not prose.
///
/// Open one before printing an eval payload and hold it until the last payload
/// line is out; the closing marker is emitted on drop, so an early `return` or
/// `?` inside the region still terminates the block correctly.
///
/// ```ignore
/// let _eval = shell_eval::EvalBlock::open();
/// println!("export AIDA_SESSION_ROLE='{}'", role);
/// ```
// trace:TASK-1171 | ai:claude
pub(crate) struct EvalBlock {
    /// Did THIS guard print the opening marker (outermost + wrapper-capable)?
    emitted: bool,
}

impl EvalBlock {
    /// Open a block. A no-op — bare payload, byte-identical to the legacy
    /// contract — when the caller's wrapper doesn't advertise the capability.
    pub(crate) fn open() -> Self {
        Self::open_with(wrapper_speaks_eval_block())
    }

    /// Testable core: `capable` is the negotiated protocol decision.
    pub(crate) fn open_with(capable: bool) -> Self {
        let outermost = DEPTH.fetch_add(1, Ordering::SeqCst) == 0;
        let emitted = capable && outermost;
        if emitted {
            println!("{EVAL_BEGIN}");
        }
        Self { emitted }
    }
}

impl Drop for EvalBlock {
    fn drop(&mut self) {
        DEPTH.fetch_sub(1, Ordering::SeqCst);
        if self.emitted {
            println!("{EVAL_END}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_probe_is_whole_token_and_case_insensitive() {
        assert!(marker_has_cap(
            Some("role,session,eval-block"),
            "eval-block"
        ));
        assert!(marker_has_cap(Some("role, Eval-Block ,dev"), "eval-block"));
        // A substring of another token must never count.
        assert!(!marker_has_cap(Some("role,eval-blockish"), "eval-block"));
        assert!(!marker_has_cap(Some(""), "eval-block"));
        assert!(!marker_has_cap(None, "eval-block"));
    }

    /// The version-skew matrix, on the binary's side of the negotiation.
    #[test]
    fn stale_or_absent_wrapper_gets_the_bare_payload() {
        // Wrapper installed before the dedicated channel existed.
        assert!(!marker_speaks_eval_block(Some(
            "role,session,dev,worktree,worktree-exit,worktree-stale"
        )));
        // No wrapper at all (raw `eval "$(aida …)"`).
        assert!(!marker_speaks_eval_block(None));
        // Current wrapper.
        assert!(marker_speaks_eval_block(Some(
            "role,session,dev,worktree,worktree-exit,worktree-stale,eval-block"
        )));
    }

    /// Markers must be shell comments: a legacy whole-stdout eval of a marked
    /// payload has to be inert, not a syntax error.
    #[test]
    fn markers_are_shell_comments() {
        assert!(EVAL_BEGIN.starts_with('#'));
        assert!(EVAL_END.starts_with('#'));
    }

    /// Nested opens collapse into one block — a second pair of markers would
    /// close the block early and leak the remaining payload into prose.
    #[test]
    fn nested_blocks_emit_a_single_marker_pair() {
        let outer = EvalBlock::open_with(true);
        assert!(outer.emitted, "the outermost guard owns the markers");
        {
            let inner = EvalBlock::open_with(true);
            assert!(!inner.emitted, "a nested guard must stay silent");
        }
        drop(outer);
        assert_eq!(DEPTH.load(Ordering::SeqCst), 0, "depth unwinds on drop");
    }
}
