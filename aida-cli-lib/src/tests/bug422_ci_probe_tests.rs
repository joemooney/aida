use super::non_tty_skips_ci_probe;

#[test]
fn skips_only_when_non_tty_default_and_probe_would_run() {
    // The bug: bare `aida session end` in a non-TTY orchestrator hangs.
    assert!(
        non_tty_skips_ci_probe(false, false, false, false),
        "non-TTY + no flags → skip the hanging probe"
    );
    // A TTY user keeps the interactive probe.
    assert!(!non_tty_skips_ci_probe(false, false, false, true));
    // Explicit --wait-ci/--watch-ci is honored even headless (caller's call).
    assert!(!non_tty_skips_ci_probe(false, false, true, false));
    // --skip-ci / --force already bypass the probe via the existing guard;
    // this predicate stays false so it doesn't double-handle them.
    assert!(!non_tty_skips_ci_probe(true, false, false, false));
    assert!(!non_tty_skips_ci_probe(false, true, false, false));
}
