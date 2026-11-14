//! Shared FAST/FULL executable lookup guard.
// trace:TASK-3791 | ai:codex

/// Keep every production subprocess on the process-wide hardened resolver.
// trace:TASK-1262 | ai:codex
#[test]
fn no_cli_source_outside_the_resolver_uses_raw_current_executable_lookup() {
    fn visit(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, files);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }

    let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../aida-cli-lib/src");
    let mut files = Vec::new();
    visit(&src_dir, &mut files);
    // trace:TASK-3429 | ai:codex — retain coverage of the extracted transport.
    visit(&src_dir.join("../../aida-mcp/src"), &mut files);
    let needle = concat!("current_", "exe()");
    let mut lib_total = 0;
    for path in files {
        let source = std::fs::read_to_string(&path).unwrap();
        let count = source.matches(needle).count();
        let is_kernel = path == src_dir.join("kernel.rs");
        let allowed = if path == src_dir.join("aida_bin.rs") {
            1
        } else if is_kernel {
            count
        } else {
            0
        };
        assert_eq!(
            count,
            allowed,
            "raw executable lookup in {}",
            path.display()
        );
        if is_kernel {
            lib_total += count;
        }
    }
    assert_eq!(lib_total, 1, "exactly one raw lookup in kernel.rs");
}

