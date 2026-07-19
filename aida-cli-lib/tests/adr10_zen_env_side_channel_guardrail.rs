//! ADR-10: CI guardrail for the `AIDA_ZEN` env side-channel invariant.
//!
//! ADR-10 makes the run's autonomy mode a first-class typed field on the phase
//! driver (`RealPhaseDriver::autonomy_mode`, resolved ONCE at dispatch via
//! `AutonomyMode::for_auto_complete_run`). The engine's IN-PROCESS zen branches
//! must consult that typed value, NOT re-read the bare `AIDA_ZEN` env var.
//!
//! `AIDA_ZEN` is KEPT — but strictly as the cross-process TRANSPORT to spawned
//! phase children + skill templates (which read it via `aida zen status` to
//! auto-resolve `kind:confirmation` prompts). There are exactly two sanctioned
//! touch-points:
//!   1. `src/zen.rs::detect()` — the ONE corroboration READER of `AIDA_ZEN`.
//!      Skills call it through `aida zen status`; it re-checks the run marker +
//!      lease every call so it cannot go stale (see the zen.rs module docs).
//!   2. The `lib.rs` dispatch SET-site — `set_var`/`remove_var(ZEN_ENV)` set
//!      the transport ONCE from the resolved typed value for children.
//!
//! This test fails if a NEW bare in-process `AIDA_ZEN` READ (`env::var` /
//! `env::var_os` of `ZEN_ENV` / `"AIDA_ZEN"`) appears anywhere outside
//! `src/zen.rs`. A set/remove is transport plumbing and is not a read, so the
//! dispatch site passes without an allow-list entry. `AIDA_ZEN_TOKEN`,
//! `AIDA_ZEN_PAUSE_ALWAYS`, and `AIDA_ZEN_NO_AI` are DIFFERENT vars and are not
//! matched — only the bare `AIDA_ZEN` flag is guarded.
//!
//! Mirrors `tests/adr9_one_engine_guardrail.rs`.
// trace:ADR-10 | ai:claude

use std::fs;
use std::path::{Path, PathBuf};

/// Source files (relative to the crate root) allowed to READ the bare
/// `AIDA_ZEN` env var. Only the corroboration reader in `zen.rs`. Adding a new
/// reader? Consult the carried typed `RealPhaseDriver::autonomy_mode` field
/// instead (that is the whole point of ADR-10), or — if you genuinely need the
/// cross-process signal in a NEW child/skill-facing surface — add the file here
/// CONSCIOUSLY with a comment explaining why it is transport, not an engine
/// in-process branch.
// trace:ADR-10 | ai:claude
const ZEN_ENV_READER_ALLOWLIST: &[&str] = &[
    "src/zen.rs", // the ONE corroboration reader (detect()); skills reach it via `aida zen status`
];

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

/// True when a source line is a bare `AIDA_ZEN` READ: an `env::var(` /
/// `env::var_os(` call referencing the `ZEN_ENV` const or the `"AIDA_ZEN"`
/// literal. Deliberately NOT matched: `set_var` / `remove_var` (transport
/// plumbing, not a read), comment lines, and the sibling vars `AIDA_ZEN_TOKEN`
/// / `AIDA_ZEN_PAUSE_ALWAYS` / `AIDA_ZEN_NO_AI` (distinct env vars, whose consts
/// are `ZEN_TOKEN_ENV` / `ZEN_PAUSE_ALWAYS_ENV` and whose literals carry a
/// trailing `_`, so neither `ZEN_ENV` nor `"AIDA_ZEN"` matches them).
fn is_bare_zen_read(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with("//") || t.starts_with('*') {
        return false;
    }
    let is_read = line.contains("env::var(") || line.contains("env::var_os(");
    if !is_read {
        return false;
    }
    // The AIDA_ZEN flag const (`ZEN_ENV`) — NOT `ZEN_TOKEN_ENV` /
    // `ZEN_PAUSE_ALWAYS_ENV`, neither of which contains the substring
    // `ZEN_ENV`. Or the bare literal `"AIDA_ZEN"` (closing quote excludes
    // `"AIDA_ZEN_TOKEN"` etc.).
    line.contains("ZEN_ENV") || line.contains("\"AIDA_ZEN\"")
}

#[test]
fn no_new_bare_aida_zen_env_read_outside_zen_rs() {
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
        if ZEN_ENV_READER_ALLOWLIST.contains(&rel.as_str()) {
            continue;
        }
        let src = fs::read_to_string(f).unwrap();
        for (i, line) in src.lines().enumerate() {
            if is_bare_zen_read(line) {
                offenders.push(format!("{rel}:{} — {}", i + 1, line.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "ADR-10 zen-side-channel invariant violated: a bare `AIDA_ZEN` env READ \
         appeared outside the sanctioned `src/zen.rs` corroboration path. The \
         engine's in-process zen branches must consult the carried typed \
         `RealPhaseDriver::autonomy_mode` field (via `is_zen_run()`), NOT re-read \
         the `AIDA_ZEN` env var — that var is KEPT only as the cross-process \
         transport to phase children / skill templates. If this is a genuine new \
         child/skill-facing corroboration reader, add its file to \
         ZEN_ENV_READER_ALLOWLIST with a comment. Offenders: {offenders:?}"
    );
}

/// The positive side: `zen.rs` must still actually read `AIDA_ZEN` (that is the
/// sanctioned corroboration reader), so the allow-list can't rot into a dead
/// entry that quietly permits removing the transport reader.
#[test]
fn zen_rs_still_reads_the_transport_env_var() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let zen_rs = fs::read_to_string(root.join("src/zen.rs")).unwrap();
    let reads = zen_rs.lines().any(is_bare_zen_read);
    assert!(
        reads,
        "ADR-10: `src/zen.rs` no longer reads `AIDA_ZEN` — the corroboration \
         reader (`detect()`) is the sanctioned cross-process transport reader \
         and must remain."
    );
}
