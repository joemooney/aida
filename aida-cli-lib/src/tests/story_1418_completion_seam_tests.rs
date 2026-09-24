//! STORY-1418: guard the into-Completed seam (`crate::completion`).
//!
//! Every path that reaches `Completed` must stamp it through the seam so the
//! `SpecCompleted` ship record cannot be forgotten by a new path. These tests
//! scan the non-test source of aida-cli-lib and aida-core and fail when a
//! direct Completed assignment appears outside the seam, or when a file stamps
//! Completed through `mark_completed` without also emitting.
//!
//! The needles are split with `concat!` so this file can never match itself.
// trace:STORY-1418 | ai:claude

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn rust_sources(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            // Out-of-line test modules are fixtures, not completion paths.
            if path.file_name().and_then(|n| n.to_str()) == Some("tests") {
                continue;
            }
            rust_sources(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn scanned_files() -> Vec<(String, String)> {
    let cli_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let core_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../aida-core/src");
    let mut out = Vec::new();
    for (label, root) in [("aida-cli-lib", cli_src), ("aida-core", core_src)] {
        let mut files = Vec::new();
        rust_sources(&root, &mut files);
        for path in files {
            let rel = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push((
                format!("{label}/{rel}"),
                std::fs::read_to_string(&path).unwrap(),
            ));
        }
    }
    out
}

/// Direct `... = RequirementStatus::Completed;` assignments and
/// `set_status_from_str("completed")` literals, counted per file.
fn direct_completed_writes(source: &str) -> usize {
    let assign = concat!("RequirementStatus::", "Completed;");
    let setter = concat!("set_status_from_str(\"", "completed\")");
    source.matches(assign).count() + source.to_ascii_lowercase().matches(setter).count()
}

/// Files allowed to write Completed directly, with the exact count. Anything
/// not listed must be zero. Raising a count here needs a reason: a new
/// production path that reaches Completed goes through `crate::completion`
/// instead, so it emits the ship record.
fn allowed_direct_writes() -> BTreeMap<&'static str, usize> {
    BTreeMap::from([
        // The seam itself.
        ("aida-cli-lib/completion.rs", 1),
        // The pure in-memory setter the seam calls (and its unit test).
        ("aida-core/models.rs", 2),
        // GitHub import creates a record born Completed from a closed issue:
        // not a transition, so no ship record.
        ("aida-cli-lib/tracker_cmd.rs", 1),
        // Inline #[cfg(test)] fixtures.
        ("aida-cli-lib/lib.rs", 1),
        ("aida-cli-lib/doctor_cmd.rs", 1),
        ("aida-cli-lib/report_cmd.rs", 1),
        // +2: TASK-1464 `--sort completed` test fixtures (inline #[cfg(test)]).
        // +5: TASK-1474 `completed_at` sort/self-heal test fixtures (inline
        // #[cfg(test)]).
        ("aida-core/db/cache.rs", 16),
        ("aida-core/db/cached_git_backend.rs", 4),
        ("aida-core/db/git_backend.rs", 1),
    ])
}

#[test]
fn no_direct_completed_write_outside_the_seam() {
    let allowed = allowed_direct_writes();
    let mut offenders = Vec::new();
    for (name, source) in scanned_files() {
        let count = direct_completed_writes(&source);
        let expected = allowed.get(name.as_str()).copied().unwrap_or(0);
        if count != expected {
            offenders.push(format!(
                "{name}: {count} direct Completed write(s), expected {expected}"
            ));
        }
    }
    assert!(
        offenders.is_empty(),
        "a path reaches Completed outside the into-Completed seam; route it through \
         crate::completion::transition_to_completed (or mark_completed + \
         emit_spec_completed) so it emits the ship record:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn every_file_that_stamps_completed_through_the_seam_also_emits() {
    let stamp = concat!("completion::mark_", "completed(");
    let emit = concat!("emit_spec_", "completed(");
    let mut offenders = Vec::new();
    for (name, source) in scanned_files() {
        if name == "aida-cli-lib/completion.rs" {
            continue;
        }
        if source.contains(stamp) && !source.contains(emit) {
            offenders.push(name);
        }
    }
    assert!(
        offenders.is_empty(),
        "these files stamp Completed via mark_completed but never emit the ship \
         record: {offenders:?}"
    );
}

/// The guard must actually see the seam and the known routed paths; an empty
/// scan (wrong root) would pass vacuously.
#[test]
fn the_scan_sees_the_routed_paths() {
    let files: BTreeMap<String, String> = scanned_files().into_iter().collect();
    let stamp = concat!("completion::mark_", "completed(");
    let transition = concat!("transition_to_", "completed(");
    assert!(files["aida-cli-lib/lib.rs"].contains(stamp));
    assert!(files["aida-cli-lib/lib.rs"].contains(transition));
    assert!(files["aida-cli-lib/queue_cmd.rs"].contains(transition));
    assert!(files["aida-cli-lib/git_backend_cmd.rs"].contains(stamp));
    assert!(files.contains_key("aida-core/models.rs"));
}

/// The seam emits exactly once on a real transition and not on a re-set.
#[test]
fn transition_to_completed_emits_only_on_a_real_transition() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();

    let mut req = aida_core::Requirement::new("seam".into(), String::new());
    req.set_status_from_str("Done");
    let mut persisted = 0;
    let into = crate::completion::transition_to_completed(
        &mut req,
        Some(root),
        "TASK-14180",
        "",
        "test",
        |r, prior| {
            assert!(matches!(r.status, aida_core::RequirementStatus::Completed));
            assert!(matches!(prior, aida_core::RequirementStatus::Done));
            persisted += 1;
            Ok(())
        },
    )
    .unwrap();
    assert!(into);
    assert_eq!(persisted, 1);

    // Re-setting an already-Completed spec is not a transition.
    let again = crate::completion::transition_to_completed(
        &mut req,
        Some(root),
        "TASK-14180",
        "",
        "test",
        |_, _| Ok(()),
    )
    .unwrap();
    assert!(!again);

    let completed: Vec<_> = crate::events::read_all(root)
        .into_iter()
        .filter(|e| {
            e.spec.as_deref() == Some("TASK-14180")
                && matches!(e.kind, crate::events::EventKind::SpecCompleted { .. })
        })
        .collect();
    assert_eq!(completed.len(), 1, "exactly one ship record");
}

/// A failed persist must not emit.
#[test]
fn a_failed_persist_does_not_emit() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    let mut req = aida_core::Requirement::new("seam".into(), String::new());
    req.set_status_from_str("Done");
    let result = crate::completion::transition_to_completed(
        &mut req,
        Some(root),
        "TASK-14181",
        "",
        "test",
        |_, _| Err(anyhow::anyhow!("write failed")),
    );
    assert!(result.is_err());
    assert!(crate::events::read_all(root)
        .iter()
        .all(|e| e.spec.as_deref() != Some("TASK-14181")));
}
