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
            out.push(path);
        }
    }
}

/// Raw `kill(…, 0)` probes on code lines before the file's `#[cfg(test)] mod`.
fn raw_probes(source: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = source.lines().collect();
    let mut hits = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let code = line.trim_start();
        if code.starts_with("#[cfg(test)]")
            && lines
                .get(i + 1)
                .is_some_and(|next| next.trim_start().starts_with("mod "))
        {
            break;
        }
        if code.starts_with("//") {
            continue;
        }
        if let Some(at) = code.find("kill(") {
            let args = &code[at..];
            if args.contains(", 0)") || args.contains(",0)") {
                hits.push((i + 1, code.to_string()));
            }
        }
    }
    hits
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
