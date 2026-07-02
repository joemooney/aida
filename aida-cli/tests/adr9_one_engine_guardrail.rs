//! ADR-9: CI guardrail for the ADR-7 "one per-spec engine" invariant.
//!
//! Every per-spec driver runs the implement -> CI -> review -> merge -> pull
//! lifecycle through the ONE engine, `auto_complete::orchestrate_with_resume`.
//! `aida zen` and `aida integrate` reach it by self-invoking
//! `aida queue work --auto-complete` (a subprocess that re-enters the engine);
//! the only in-process production call site is the `queue work` handler in
//! `main.rs`. `aida burndown` is the SINGLE sanctioned FLEET-layer exception —
//! it fans out native subagents one level above the per-spec lifecycle and
//! deliberately does NOT route through the engine.
//!
//! This test converts that invariant from prose (which failed at zen slice-1,
//! PR #1231: implement+PR with zero review) into a CI gate. See
//! docs/architecture/one-engine-invariant.md.
// trace:ADR-9 | ai:claude

use std::fs;
use std::path::{Path, PathBuf};

/// Source files (relative to the crate root) allowed to call the engine
/// `orchestrate_with_resume` in PRODUCTION code. Adding a new per-spec driver?
/// Route it through the engine — either self-invoke `queue work --auto-complete`
/// (like zen/integrate, which adds NO new call site here) or, if it is a genuine
/// new in-process engine entry point, add its file here CONSCIOUSLY. Do NOT
/// reimplement the per-spec lifecycle: that is the burndown fleet-layer exception,
/// and it is the only one (see the architecture doc).
// trace:ADR-9 | ai:claude
const ENGINE_CALLER_ALLOWLIST: &[&str] = &[
    "src/auto_complete.rs", // the engine module itself (the orchestrate wrapper)
    "src/main.rs",          // the `aida queue work --auto-complete` handler
];

/// The engine entry point every per-spec driver must route through.
const ENGINE_FN: &str = "orchestrate_with_resume(";

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

/// Count production (non-comment) call lines of the engine entry point, skipping
/// the `fn orchestrate_with_resume` definition itself.
fn production_engine_calls(src: &str) -> usize {
    src.lines()
        .filter(|line| {
            let t = line.trim_start();
            line.contains(ENGINE_FN)
                && !t.starts_with("//")
                && !t.starts_with('*')
                && !line.contains("fn orchestrate_with_resume")
        })
        .count()
}

#[test]
fn only_allowlisted_files_call_the_one_engine_in_production() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rs_files(&root.join("src"), &mut files);
    assert!(!files.is_empty(), "found no src/*.rs — test harness broken");

    let mut offenders = Vec::new();
    for f in &files {
        let rel = f
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if ENGINE_CALLER_ALLOWLIST.contains(&rel.as_str()) {
            continue;
        }
        let src = fs::read_to_string(f).unwrap();
        let n = production_engine_calls(&src);
        if n > 0 {
            offenders.push(format!("{rel} ({n} call site(s))"));
        }
    }

    assert!(
        offenders.is_empty(),
        "ADR-9 one-engine invariant violated: these files call \
         `orchestrate_with_resume` outside the allow-list. A per-spec driver must \
         route the lifecycle through the ONE engine — self-invoke \
         `aida queue work --auto-complete` (like zen/integrate), or, if this is a \
         genuine new in-process engine entry, add the file to \
         ENGINE_CALLER_ALLOWLIST with a comment. Do NOT reimplement the per-spec \
         lifecycle (burndown is the only sanctioned fleet-layer exception; see \
         docs/architecture/one-engine-invariant.md). Offenders: {offenders:?}"
    );
}

/// The positive side of the invariant: the allow-listed `main.rs` handler must
/// still actually route through the engine, so the allow-list can't rot into a
/// dead entry that quietly permits a bypass.
#[test]
fn the_queue_work_handler_still_routes_through_the_engine() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let main_rs = fs::read_to_string(root.join("src/main.rs")).unwrap();
    assert!(
        production_engine_calls(&main_rs) >= 1,
        "ADR-9: `main.rs` no longer calls `orchestrate_with_resume` — the \
         `queue work --auto-complete` handler must route through the one engine."
    );
}
