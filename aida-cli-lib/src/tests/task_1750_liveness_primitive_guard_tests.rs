//! TASK-1750: process liveness has ONE primitive, `aida_core::liveness::pid_is_alive`
//! (and `process_identity_is_alive` when a start time was recorded). A raw
//! `kill(pid, 0)` in production code is a second definition of "alive" that skips the
//! zombie check (BUG-1741) and the negative-pid wrap, so leases, registries and
//! watchdogs drift apart again. This guard scans every crate's production source.
// trace:TASK-1750 | ai:claude

use std::path::{Path, PathBuf};

const CRATES: &[&str] = &[
    "aida-core",
    "aida-cli-lib",
    "aida-cli",
    "aida-server",
    "aida-tui",
];

/// (path suffix, why) — the only production sites allowed a raw `kill(…, 0)`.
const ALLOWED: &[(&str, &str)] = &[
    (
        "aida-core/src/liveness.rs",
        "the canonical primitive itself",
    ),
    (
        "aida-cli-lib/src/exit_signal.rs",
        "a process-GROUP probe (kill(-pgid, 0)), not single-pid liveness",
    ),
    // trace:STORY-2338 | ai:claude
    (
        "aida-cli-lib/src/deputy_shell/runner.rs",
        "a process-GROUP probe (kill(-pgid, 0)), not single-pid liveness",
    ),
    // trace:TASK-2886 | ai:codex
    (
        "aida-cli-lib/src/test_home.rs",
        "test-home cleanup requires namespace-local ESRCH; ancestor-mounted procfs \
         cannot establish local PID death. Retain zombies and unknown errors, \
         and only probe homes with matching boot and PID-namespace identity",
    ),
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Integration and unit-test files may probe processes however they like.
            if path.file_name().is_some_and(|n| n == "tests") {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            // Dedicated cfg(test) module files contain fixture observations,
            // not production decisions. trace:STORY-2537 | ai:codex
            let name = path.file_name().unwrap().to_string_lossy();
            if name == "tests.rs" || name.ends_with("_tests.rs") {
                continue;
            }
            out.push(path);
        }
    }
}

/// Raw `kill(…, 0)` probes on code lines before the file's `#[cfg(test)] mod`.
fn production_source(source: &str) -> String {
    let mut out = String::new();
    let lines: Vec<_> = source.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.trim() == "#[cfg(test)]"
            && lines
                .get(i + 1)
                .is_some_and(|l| l.trim_start().starts_with("mod "))
        {
            break;
        }
        if !line.trim_start().starts_with("//") {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

fn raw_probes(source: &str) -> Vec<(usize, String)> {
    let source = production_source(source);
    let pattern = regex::Regex::new(r"\bkill\s*\(\s*[^,;{}]+,\s*0\s*\)").unwrap();
    pattern
        .find_iter(&source)
        .map(|hit| {
            (
                source[..hit.start()]
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count()
                    + 1,
                hit.as_str().to_string(),
            )
        })
        .collect()
}

// trace:STORY-2537 trace:BUG-2458 | ai:codex
// Enumerated temporary compatibility sites. These consumers retain their own
// policy until their migration slice; counts may fall, never rise. Registry
// row, strict mail-wake and explicit-bind capture sites migrated in S4.
// trace:TASK-3366 | ai:codex
// The primitive files themselves are explicit exclusions. This inventory grants no owner authority.
const IDENTITY_PATTERN: &str = r"\b(?:pid_is_alive|process_identity_is_alive|process_identity_matches_strict|process_provably_dead|process_start_identity(?:_exact)?|parse_proc_stat_starttime|parse_start_identity|pid_namespace|recorded_lease_owner)\s*\(|\.start_time\s*\(|\.probe\s*\(";

#[test]
fn proc_identity_legacy_decisions_do_not_expand() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let baseline: std::collections::BTreeMap<String, usize> =
        serde_json::from_str(include_str!("fixtures/proc_identity_legacy_baseline.json")).unwrap();
    let pattern = regex::Regex::new(IDENTITY_PATTERN).unwrap();
    let mut files = Vec::new();
    for krate in CRATES {
        rust_files(&root.join(krate).join("src"), &mut files);
    }
    let mut offenders = Vec::new();
    for file in files {
        let rel = file.strip_prefix(root).unwrap().to_str().unwrap();
        if [
            "aida-core/src/proc_identity.rs",
            "aida-core/src/process_probe.rs",
            "aida-core/src/liveness.rs",
            "aida-core/src/pid_observer.rs",
        ]
        .contains(&rel)
        {
            continue;
        }
        let count = pattern
            .find_iter(&production_source(&std::fs::read_to_string(&file).unwrap()))
            .count();
        if count > baseline.get(rel).copied().unwrap_or(0) {
            offenders.push(format!("{rel}: {count} identity decisions"));
        }
    }
    assert!(offenders.is_empty(), "new private process decisions must use proc_identity; migrate an existing site to lower its baseline:\n{}", offenders.join("\n"));
}

#[test]
fn proc_identity_ratchet_recognizes_alternate_syntax() {
    assert_eq!(
        raw_probes("unsafe { libc::kill \n ( pid ,\n0 ) };").len(),
        1
    );
    let pattern = regex::Regex::new(IDENTITY_PATTERN).unwrap();
    assert_eq!(
        pattern
            .find_iter(
                "process_identity_is_alive \n (pid, start); p.start_time (); source.probe (id);"
            )
            .count(),
        3
    );
}

#[test]
fn production_code_has_no_raw_kill_zero_liveness_probe() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let mut files = Vec::new();
    for krate in CRATES {
        rust_files(&root.join(krate).join("src"), &mut files);
    }
    assert!(
        files
            .iter()
            .any(|f| f.ends_with("aida-core/src/liveness.rs")),
        "the scan must reach the canonical primitive, or it is scanning nothing"
    );

    let mut offenders = Vec::new();
    for file in &files {
        let rel = file.strip_prefix(root).unwrap_or(file);
        if ALLOWED.iter().any(|(suffix, _)| rel.ends_with(suffix)) {
            continue;
        }
        let source = std::fs::read_to_string(file).expect("read source");
        for (line, code) in raw_probes(&source) {
            offenders.push(format!("{}:{line}: {code}", rel.display()));
        }
    }
    assert!(
        offenders.is_empty(),
        "raw kill(pid, 0) liveness probes found; call aida_core::liveness::pid_is_alive \
         (or process_identity_is_alive) instead:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn raw_probe_scan_sees_code_and_skips_comments_and_test_modules() {
    let source = "\
fn a() { unsafe { libc::kill(pid, 0) }; }
// libc::kill(pid, 0) in a comment
#[cfg(test)]
mod tests {
    fn b() { unsafe { libc::kill(pid, 0) }; }
}
";
    let hits = raw_probes(source);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].0, 1);
}
