//! TASK-1169 / ADR-22: CI guardrail for the "launcher-set, never operator-set"
//! half of the ratified integration-wait design.
//!
//! The 2026-07-18 limp incident came from a hand-typed `aida burndown run`
//! lacking the background-wait ceiling env that the daemonized launch had. The
//! fix is that AIDA sets the ceiling on EVERY headless child it spawns, so who
//! typed the launch cannot change the behaviour. That is an invariant about
//! spawn sites, and prose does not enforce it — this test does: every headless
//! agent spawn must apply the ceiling alongside its other env, and nothing may
//! set the retired `=0` (wait-forever) stopgap.
// trace:TASK-1169 | ai:claude

use std::fs;
use std::path::{Path, PathBuf};

/// Markers that identify a headless agent spawn in production code:
///   - `AIDA_HEADLESS` — the central `session.rs` spawn + exec paths every
///     orchestrator phase goes through.
///   - `AIDA_BURNDOWN_LOCK_HELD` — the two `aida burndown run` launch sites
///     (quiet + `--verbose`).
const SPAWN_MARKERS: &[&str] = &["\"AIDA_HEADLESS\", \"1\"", "AIDA_BURNDOWN_LOCK_HELD"];

/// The ceiling application each spawn site must carry, within a short window
/// of its marker (same `Command` builder chain).
const CEILING_MARKER: &str = "ceiling_key";

/// How many lines after a spawn marker still count as the same builder chain.
const WINDOW: usize = 12;

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}

/// Production sources only — test fixtures may legitimately mention either
/// marker without spawning anything.
fn production_sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rs_files(&root, &mut files);
    files.retain(|p| {
        !p.components()
            .any(|c| c.as_os_str() == "tests" || c.as_os_str() == "testdata")
    });
    files
}

#[test]
fn every_headless_spawn_site_sets_the_background_wait_ceiling() {
    let mut violations: Vec<String> = Vec::new();
    for path in production_sources() {
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        let lines: Vec<&str> = body.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            // `.env(` narrows this to a CHILD-process builder chain: an inline
            // test that `set_var`s the same name in its own process is not a
            // spawn site and has no child to bound.
            if !line.contains(".env(") || !SPAWN_MARKERS.iter().any(|m| line.contains(m)) {
                continue;
            }
            let end = (i + WINDOW).min(lines.len());
            let window = lines[i..end].join("\n");
            if !window.contains(CEILING_MARKER) {
                violations.push(format!(
                    "{}:{} — a headless spawn that does not set the background-wait ceiling",
                    path.display(),
                    i + 1
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "TASK-1169/ADR-22: the LAUNCHER sets the background-wait ceiling on every headless \
         child, so a hand-typed launch behaves identically to a daemonized one. These spawn \
         sites don't:\n{}",
        violations.join("\n")
    );
}

/// The `=0` stopgap removes the safety valve entirely: a genuinely wedged task
/// hangs the drain forever. The resolver clamps configuration up to a floor;
/// this makes sure no code path hard-codes the zero either.
#[test]
fn no_production_code_sets_the_retired_wait_forever_stopgap() {
    let mut violations: Vec<String> = Vec::new();
    for path in production_sources() {
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        for (i, line) in body.lines().enumerate() {
            let squashed: String = line.chars().filter(|c| !c.is_whitespace()).collect();
            if squashed.contains("CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS\",\"0\"")
                || squashed.contains("CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS=0")
            {
                violations.push(format!("{}:{}", path.display(), i + 1));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "TASK-1169/ADR-22: wait-forever is the retired stopgap — use the bounded ceiling \
         (a real ceiling that rarely fires beats no ceiling). Found at:\n{}",
        violations.join("\n")
    );
}
