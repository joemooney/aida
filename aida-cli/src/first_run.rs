//! Passive first-run hint chain — walks a brand-new user through the core
//! loop (file a spec → implement + trace → review → merge → the spec
//! auto-completes) as contextual nudges that surface *automatically* as each
//! step is performed, never as an opt-in command the user has to know to run.
//!
//! Design (STORY-700):
//!   1. PASSIVE + CONTEXTUAL. Each hint fires from the handler of the command
//!      that just finished the prior step (`aida init`, the first `aida add`,
//!      the first `aida queue done`, the `aida pull` that auto-completes the
//!      spec). There is deliberately NO `aida tour` command (operator binding
//!      decision).
//!   2. LINEAR STEP MACHINE. The arc's progress is one integer persisted in
//!      per-clone runtime state (`.aida/first_run.json`, gitignored by the
//!      deny-by-default `.aida/*` rule). Each step advances the integer and
//!      prints exactly one hint. A step only fires when the machine is sitting
//!      at the immediately-prior step, so nothing re-nags and the completed arc
//!      silences the whole chain.
//!   3. IDEMPOTENT. Re-running `aida init` never replays a started-or-finished
//!      arc (the state file already exists → no-op). Every advance is a
//!      compare-then-set, so a repeated `aida add` / `queue done` / `pull`
//!      fires nothing once its step has passed.
//!   4. ADVISORY + NON-BLOCKING. Hints go to stderr, never change exit codes,
//!      and any state-file I/O failure degrades to silence.
//!   5. SILENCEABLE. One knob — `AIDA_FIRST_RUN_HINTS` env or `[hints]
//!      first_run` in `.aida/config.toml`. Also auto-silent off a TTY
//!      (scripts / CI / headless get ZERO output), and folded under the global
//!      `AIDA_HINTS` umbrella so disabling all hints disables these too.
//!   6. USER-FACING COPY. The hint strings carry NO spec-ids
//!      (docs/user-facing-text-conventions.md) — they teach the *shape* of the
//!      loop, not one spec's identifiers.
//!
//! trace:STORY-700 | ai:claude

use std::io::IsTerminal;
use std::path::Path;

/// The arc's steps, stored as the integer of the LAST hint emitted. The
/// machine only advances from `N` to `N+1` when the triggering command runs
/// while sitting at `N`, so each hint fires exactly once and in order.
pub const STEP_NONE: u8 = 0; // arc not started
pub const STEP_INIT: u8 = 1; // init hint emitted → waiting for the first spec
pub const STEP_SPEC_FILED: u8 = 2; // first-spec hint emitted → waiting for `queue done`
pub const STEP_WORK_DONE: u8 = 3; // work-done hint emitted → waiting for the merge auto-bump
pub const STEP_COMPLETED: u8 = 4; // payoff emitted → arc COMPLETE (chain silenced)

/// Basename of the per-clone runtime state file under `.aida/`. Untracked by
/// convention (deny-by-default `.aida/*`), so no `!.aida/...` allow-list entry
/// is needed.
const STATE_BASENAME: &str = "first_run.json";

fn state_path(project_root: &Path) -> std::path::PathBuf {
    project_root.join(".aida").join(STATE_BASENAME)
}

/// Read the persisted step. A missing / unreadable / unparseable file reads as
/// `STEP_NONE` (arc not started) — we never let state I/O fail a command.
pub fn read_step(project_root: &Path) -> u8 {
    let raw = match std::fs::read_to_string(state_path(project_root)) {
        Ok(s) => s,
        Err(_) => return STEP_NONE,
    };
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|v| v.get("step").and_then(|s| s.as_u64()))
        .map(|n| n as u8)
        .unwrap_or(STEP_NONE)
}

/// Persist the step. Best-effort — a write failure is swallowed (the hint is a
/// nicety, never load-bearing). Creates `.aida/` if somehow absent.
fn write_step(project_root: &Path, step: u8) {
    let path = state_path(project_root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let body = format!("{{\n  \"step\": {}\n}}\n", step);
    // Atomic write — uniform with the other `.aida/` runtime writers.
    let _ = aida_core::write_atomic(&path, body);
}

/// Whether the first-run chain may print. One knob, checked in order:
/// `AIDA_FIRST_RUN_HINTS` env (`false`/`0`/`no`/`off` → off;
/// `true`/`1`/`yes`/`on` → on), then `[hints] first_run` in
/// `.aida/config.toml` (`false` → off), else default → on. Finally the global
/// workflow-hints umbrella (`AIDA_HINTS` / `[hints] workflow_hints`) applies:
/// if all hints are disabled, these are too.
///
/// This is the pure not-a-TTY-agnostic decision; call sites additionally gate
/// on [`tty`] so scripts / CI / headless runs stay silent.
pub fn enabled(project_root: &Path) -> bool {
    if let Ok(raw) = std::env::var("AIDA_FIRST_RUN_HINTS") {
        match raw.trim().to_ascii_lowercase().as_str() {
            "false" | "0" | "no" | "off" => return false,
            "true" | "1" | "yes" | "on" => return true,
            _ => {}
        }
    }
    if let Some(false) = read_config_first_run(project_root) {
        return false;
    }
    // Umbrella: the global hints toggle silences the first-run chain too.
    crate::workflow_hints::enabled(Some(project_root))
}

/// Parse `[hints] first_run = true/false` from `.aida/config.toml`. `None` when
/// the file / section / key is absent. Same line-scanner shape as
/// `workflow_hints::read_config_workflow_hints`.
fn read_config_first_run(project_root: &Path) -> Option<bool> {
    let config_path = project_root.join(".aida").join("config.toml");
    let content = std::fs::read_to_string(&config_path).ok()?;
    let mut in_hints = false;
    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix('[') {
            in_hints = stripped.trim_end_matches(']').trim() == "hints";
            continue;
        }
        if in_hints {
            if let Some((key, val)) = line.split_once('=') {
                if key.trim() == "first_run" {
                    let v = val
                        .split('#')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'');
                    return match v.to_ascii_lowercase().as_str() {
                        "true" => Some(true),
                        "false" => Some(false),
                        _ => None,
                    };
                }
            }
        }
    }
    None
}

/// Whether stderr is an interactive terminal — the auto-silent gate for
/// scripts / CI / headless runs (criterion 5). Split out so callers read as
/// `enabled(root) && tty()`.
pub fn tty() -> bool {
    std::io::stderr().is_terminal()
}

// ---- Hint copy (pure, unit-testable, NO spec-ids) -------------------------

/// The hint printed for a given step. Each names the single next command and
/// carries NO spec-ids (criterion 6). `STEP_NONE` has no copy.
pub fn hint_lines(step: u8) -> Vec<String> {
    match step {
        STEP_INIT => vec![
            "AIDA is ready. File your first spec to start the loop:".to_string(),
            "  aida add --title \"...\"".to_string(),
        ],
        STEP_SPEC_FILED => vec![
            "Spec filed. Now build it — and drop a `// trace:` comment where you \
             implement it so the code links back."
                .to_string(),
            "When it's finished on a branch, mark it done:".to_string(),
            "  aida queue done".to_string(),
        ],
        STEP_WORK_DONE => vec![
            "Marked done. Open a pull request, get it reviewed, and merge it once \
             it's green."
                .to_string(),
            "Then sync — the merge auto-completes the spec:".to_string(),
            "  aida pull".to_string(),
        ],
        STEP_COMPLETED => vec![
            "That's the loop: the spec you filed just reached Completed — the merge \
             completed it automatically, no manual status change."
                .to_string(),
            "You've gone spec → code → review → merge → done. That's AIDA.".to_string(),
        ],
        _ => Vec::new(),
    }
}

/// Emit the hint for `step` to stderr with a dimmed "Getting started:" prefix
/// (distinct from `workflow_hints`' "Workflow hint:" label so the two chains
/// read differently). No-op for a step with no copy.
fn emit(step: u8) {
    let lines = hint_lines(step);
    if lines.is_empty() {
        return;
    }
    use colored::Colorize;
    let prefix = crate::glyph(crate::glyphs::Glyph::Info).dimmed();
    let label = "Getting started:".dimmed();
    eprintln!("\n{} {} {}", prefix, label, lines[0].dimmed());
    for line in &lines[1..] {
        eprintln!("  {}", line.dimmed());
    }
}

// ---- Pure advance decision (unit-testable without I/O) --------------------

/// Given the persisted `current` step and the step a handler wants to advance
/// TO (`target`), return `Some(target)` when the machine should advance (it is
/// sitting exactly one step behind), else `None` (already at/past this step —
/// no re-nag). The one exception is [`STEP_INIT`]: init only starts the arc
/// when it has NOT started (`current == STEP_NONE`), so a re-run over a
/// started-or-finished arc never replays (criterion 4).
pub fn should_advance(current: u8, target: u8) -> Option<u8> {
    if target == STEP_INIT {
        return (current == STEP_NONE).then_some(STEP_INIT);
    }
    (current + 1 == target).then_some(target)
}

/// Load state, decide via [`should_advance`], and on a hit persist + emit.
/// Fully gated: silent when disabled or off a TTY. Best-effort throughout.
fn advance_to(project_root: &Path, target: u8) {
    if !tty() || !enabled(project_root) {
        return;
    }
    let current = read_step(project_root);
    if let Some(next) = should_advance(current, target) {
        write_step(project_root, next);
        emit(next);
    }
}

// ---- Public entry points (one per command anchor) -------------------------

/// After `aida init` finishes: start the arc and print the "file your first
/// spec" hint. No-op if the arc already started (idempotent re-init).
pub fn after_init(project_root: &Path) {
    advance_to(project_root, STEP_INIT);
}

/// After the first `aida add`: advance to the implement + trace + `queue done`
/// hint. Fires only while the machine sits at [`STEP_INIT`].
pub fn after_first_spec(project_root: &Path) {
    advance_to(project_root, STEP_SPEC_FILED);
}

/// After the first `aida queue done`: advance to the review → merge → `pull`
/// hint. Fires only while the machine sits at [`STEP_SPEC_FILED`].
pub fn after_work_done(project_root: &Path) {
    advance_to(project_root, STEP_WORK_DONE);
}

/// After an `aida pull` that auto-completed the spec (Done → Completed): print
/// the payoff and terminate the arc. Fires only while the machine sits at
/// [`STEP_WORK_DONE`].
pub fn after_spec_completed(project_root: &Path) {
    advance_to(project_root, STEP_COMPLETED);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Serialize tests mutating the env knobs (cargo runs tests in-process,
    /// parallel).
    fn with_env<R>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> R) -> R {
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap();
        let prev: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| (k.to_string(), std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        let out = f();
        for (k, v) in prev {
            match v {
                Some(v) => std::env::set_var(&k, v),
                None => std::env::remove_var(&k),
            }
        }
        out
    }

    fn write_config(dir: &Path, body: &str) {
        let aida = dir.join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        std::fs::write(aida.join("config.toml"), body).unwrap();
    }

    // The step machine advances strictly one step at a time, only from the
    // immediately-prior step, and never past the terminal.
    #[test]
    fn should_advance_is_strictly_sequential() {
        // init only starts from the not-started state.
        assert_eq!(should_advance(STEP_NONE, STEP_INIT), Some(STEP_INIT));
        assert_eq!(should_advance(STEP_INIT, STEP_INIT), None); // re-init: no replay
        assert_eq!(should_advance(STEP_COMPLETED, STEP_INIT), None); // finished: no replay

        // Each later step fires only from exactly one behind.
        assert_eq!(
            should_advance(STEP_INIT, STEP_SPEC_FILED),
            Some(STEP_SPEC_FILED)
        );
        assert_eq!(should_advance(STEP_NONE, STEP_SPEC_FILED), None); // skipped init → no fire
        assert_eq!(should_advance(STEP_SPEC_FILED, STEP_SPEC_FILED), None); // no re-nag

        assert_eq!(
            should_advance(STEP_SPEC_FILED, STEP_WORK_DONE),
            Some(STEP_WORK_DONE)
        );
        assert_eq!(should_advance(STEP_WORK_DONE, STEP_WORK_DONE), None);

        assert_eq!(
            should_advance(STEP_WORK_DONE, STEP_COMPLETED),
            Some(STEP_COMPLETED)
        );
        assert_eq!(should_advance(STEP_COMPLETED, STEP_COMPLETED), None); // arc done, silenced
    }

    // Full arc drives cleanly through the state file, once each.
    #[test]
    fn arc_progresses_once_per_step() {
        let td = TempDir::new().unwrap();
        let root = td.path();
        std::fs::create_dir_all(root.join(".aida")).unwrap();

        assert_eq!(read_step(root), STEP_NONE);

        // init starts the arc.
        write_step(root, should_advance(read_step(root), STEP_INIT).unwrap());
        assert_eq!(read_step(root), STEP_INIT);
        // re-init does not replay.
        assert!(should_advance(read_step(root), STEP_INIT).is_none());

        write_step(
            root,
            should_advance(read_step(root), STEP_SPEC_FILED).unwrap(),
        );
        assert_eq!(read_step(root), STEP_SPEC_FILED);

        write_step(
            root,
            should_advance(read_step(root), STEP_WORK_DONE).unwrap(),
        );
        assert_eq!(read_step(root), STEP_WORK_DONE);

        write_step(
            root,
            should_advance(read_step(root), STEP_COMPLETED).unwrap(),
        );
        assert_eq!(read_step(root), STEP_COMPLETED);

        // Every anchor is now a no-op — completed arc is silenced.
        assert!(should_advance(read_step(root), STEP_SPEC_FILED).is_none());
        assert!(should_advance(read_step(root), STEP_WORK_DONE).is_none());
        assert!(should_advance(read_step(root), STEP_COMPLETED).is_none());
    }

    // CRITICAL (criterion 6): no hint string leaks a spec-id. A spec-id looks
    // like `ABC-123` — an uppercase type prefix, a dash, digits.
    #[test]
    fn no_hint_contains_a_spec_id() {
        let spec_id = regex_lite_spec_id();
        for step in [STEP_INIT, STEP_SPEC_FILED, STEP_WORK_DONE, STEP_COMPLETED] {
            for line in hint_lines(step) {
                assert!(
                    !spec_id(&line),
                    "first-run hint for step {step} leaked a spec-id-shaped token: {line:?}"
                );
            }
        }
    }

    /// A dependency-free spec-id detector: matches `<UPPER>+-<DIGIT>+` (e.g.
    /// `TASK-1`, `STORY-700`, `BUG-42`), the shape a real spec-id takes.
    fn regex_lite_spec_id() -> impl Fn(&str) -> bool {
        |s: &str| {
            let bytes: Vec<char> = s.chars().collect();
            let n = bytes.len();
            let mut i = 0;
            while i < n {
                // scan an uppercase run
                let start = i;
                while i < n && bytes[i].is_ascii_uppercase() {
                    i += 1;
                }
                if i > start && i < n && bytes[i] == '-' {
                    let after_dash = i + 1;
                    let mut j = after_dash;
                    while j < n && bytes[j].is_ascii_digit() {
                        j += 1;
                    }
                    if j > after_dash {
                        return true; // UPPER+ '-' DIGIT+
                    }
                }
                if i == start {
                    i += 1; // no uppercase run consumed; step forward
                }
            }
            false
        }
    }

    // Each active step's copy names the single next command it should.
    #[test]
    fn hints_name_the_next_command() {
        assert!(hint_lines(STEP_INIT).iter().any(|l| l.contains("aida add")));
        assert!(hint_lines(STEP_SPEC_FILED)
            .iter()
            .any(|l| l.contains("aida queue done")));
        assert!(hint_lines(STEP_SPEC_FILED)
            .iter()
            .any(|l| l.contains("// trace:")));
        assert!(hint_lines(STEP_WORK_DONE)
            .iter()
            .any(|l| l.contains("aida pull")));
        // The payoff makes Completed explicit (criterion 3).
        assert!(hint_lines(STEP_COMPLETED)
            .iter()
            .any(|l| l.contains("Completed")));
        assert!(hint_lines(STEP_NONE).is_empty());
    }

    // The single silencing knob: env off / config off / global umbrella off.
    #[test]
    fn silenceable_via_the_one_knob() {
        let td = TempDir::new().unwrap();
        let root = td.path();
        std::fs::create_dir_all(root.join(".aida")).unwrap();

        // Default → enabled.
        with_env(
            &[("AIDA_FIRST_RUN_HINTS", None), ("AIDA_HINTS", None)],
            || assert!(enabled(root)),
        );
        // Dedicated env off.
        with_env(
            &[
                ("AIDA_FIRST_RUN_HINTS", Some("false")),
                ("AIDA_HINTS", None),
            ],
            || assert!(!enabled(root)),
        );
        with_env(
            &[("AIDA_FIRST_RUN_HINTS", Some("0")), ("AIDA_HINTS", None)],
            || assert!(!enabled(root)),
        );
        // Config off.
        write_config(root, "[hints]\nfirst_run = false\n");
        with_env(
            &[("AIDA_FIRST_RUN_HINTS", None), ("AIDA_HINTS", None)],
            || assert!(!enabled(root)),
        );
        // Env true beats config false.
        with_env(
            &[("AIDA_FIRST_RUN_HINTS", Some("true")), ("AIDA_HINTS", None)],
            || assert!(enabled(root)),
        );
        // Global umbrella off silences the first-run chain too.
        write_config(root, "");
        with_env(
            &[
                ("AIDA_FIRST_RUN_HINTS", None),
                ("AIDA_HINTS", Some("false")),
            ],
            || assert!(!enabled(root)),
        );
    }

    // Unparseable / missing state reads as not-started, never panics.
    #[test]
    fn corrupt_state_reads_as_not_started() {
        let td = TempDir::new().unwrap();
        let root = td.path();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        assert_eq!(read_step(root), STEP_NONE); // missing
        std::fs::write(state_path(root), b"not json{{").unwrap();
        assert_eq!(read_step(root), STEP_NONE); // garbage
        std::fs::write(state_path(root), br#"{"other": 1}"#).unwrap();
        assert_eq!(read_step(root), STEP_NONE); // no step key
    }
}
