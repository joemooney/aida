//! TASK-2712: the native Codex write set's pure parts, and the ratchet that
//! every Codex spawn path gets it from the one shared helper. The write set
//! against a real seat clone is tested beside the BUG-2169 clone fixture
//! (`bug_2169_contained_clone_bwrap_tests.rs`), the publication end to end
//! beside the seat-publish fixture (`bug_home_038_seat_publish_tests.rs`).
//! trace:TASK-2712 | ai:claude

use super::*;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// Outside a git repository there are no write roots, so the posture carries
/// no `--add-dir` at all: the write set fails closed.
#[test]
fn no_write_roots_means_the_bare_posture() {
    let flags = codex_native_sandbox_flags_for_roots(&[]).unwrap();
    assert_eq!(flags, s(&CODEX_NATIVE_POSTURE));
    assert!(carries_native_posture(&flags));
}

/// A tree outside git rescopes to the bare posture; the `--add-dir` run the
/// helper emitted is replaced, the arguments around it are kept, and an
/// argv without the native posture is never touched.
#[test]
fn rescope_replaces_only_the_emitted_add_dir_run() {
    let tmp = tempfile::tempdir().unwrap();
    let mut args = s(&[
        "--no-daemon",
        "--sandbox",
        "workspace-write",
        "--ask-for-approval",
        "never",
        "--add-dir",
        "/old/main/.aida-store",
        "--add-dir",
        "/old/main/.aida/cache-db",
        "--model",
        "m",
    ]);
    rescope_codex_native_flags(&mut args, tmp.path()).unwrap();
    assert_eq!(
        args,
        s(&[
            "--no-daemon",
            "--sandbox",
            "workspace-write",
            "--ask-for-approval",
            "never",
            "--model",
            "m",
        ])
    );

    let mut other = s(&[
        "--dangerously-bypass-approvals-and-sandbox",
        "--add-dir",
        "/x",
    ]);
    let before = other.clone();
    rescope_codex_native_flags(&mut other, tmp.path()).unwrap();
    assert_eq!(other, before);
}

/// A plan lane (BUG-2394 read-only) drops the write set with the posture.
#[test]
fn a_read_only_plan_lane_drops_every_add_dir() {
    let args = crate::session::codex_read_only_args(s(&[
        "--sandbox",
        "workspace-write",
        "--ask-for-approval",
        "never",
        "--add-dir",
        "/m/.aida-store",
        "--add-dir=/m/.aida/cache-db",
        "exec",
        "p",
    ]));
    assert_eq!(
        args,
        s(&[
            "--sandbox",
            "read-only",
            "--ask-for-approval",
            "never",
            "exec",
            "p"
        ])
    );
}

/// N1: only a native Codex lane is refused publication outside a clone, and
/// the refusal names the reason and the way out.
#[test]
fn publication_outside_a_clone_is_refused_only_for_native_codex() {
    let tmp = tempfile::tempdir().unwrap();
    let err = refuse_native_publication_outside_clone(Some("native"), tmp.path())
        .unwrap_err()
        .to_string();
    assert!(err.contains("native Codex sandbox"), "{err}");
    assert!(err.contains("seat clone"), "{err}");
    assert!(err.contains("untouched"), "{err}");
    for posture in [None, Some("aida"), Some("none")] {
        refuse_native_publication_outside_clone(posture, tmp.path()).unwrap();
    }
}

/// The source of `src/<file>`.
fn production_source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// N4 ratchet: the native posture is spelled once, here; every Codex spawn
/// path gets its argv from this module, directly or through
/// `tool_contained_flags("codex")`, which delegates to it; and the detached
/// `--bg` lane (`agent_codex_background.rs`) rescopes the set to its own tree
/// and asks for a seat clone.
#[test]
fn every_codex_spawn_path_gets_the_shared_write_set() {
    let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut stack = vec![src_dir.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != "tests") {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let rel = path
                .strip_prefix(&src_dir)
                .unwrap()
                .to_string_lossy()
                .to_string();
            if rel == "codex_native_sandbox.rs" || rel.ends_with("_tests.rs") {
                continue;
            }
            let src = production_source(&rel);
            let flat: String = src.split_whitespace().collect();
            // An argv that spells the posture itself instead of asking the
            // helper. `config_cmd.rs` renders it for display only.
            let builds = flat.contains("\"--sandbox\".to_string(),\"workspace-write\"")
                || (rel != "config_cmd.rs"
                    && flat.contains("\"--sandbox\".into(),\"workspace-write\""));
            if builds {
                offenders.push(rel);
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these files build the native Codex posture without the shared write set \
         (use crate::codex_native_sandbox): {offenders:?}"
    );

    let launch: String = production_source("agent_launch.rs")
        .split_whitespace()
        .collect();
    assert!(
        launch.contains("\"codex\"=>crate::codex_native_sandbox::codex_native_sandbox_flags()?"),
        "tool_contained_flags(\"codex\") must delegate to the shared helper"
    );
    assert!(
        launch.contains("crate::codex_native_sandbox::rescope_codex_native_flags("),
        "rescope_contained_settings must rescope a Codex launch"
    );
    let bg: String = production_source("agent_codex_background.rs")
        .split_whitespace()
        .collect();
    assert!(
        bg.contains(
            "crate::agent_launch::rescope_contained_settings(&mutconfig,&plan.launch_cwd)?"
        ),
        "the --bg lane must rescope its write set to its own tree"
    );
    assert!(
        bg.contains(
            "prepare_agent_launch_attempt(&project_root,role,spec,\"codex\",name,cwd_ref,true)?"
        ),
        "the --bg lane must ask for a seat clone"
    );
}
