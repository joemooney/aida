#![cfg(target_os = "linux")]
//! The shipped CLI and actual Agy hook shape must deliver across vendors.
// trace:STORY-1843.ac0eef56 | ai:codex
#[test]
fn story_1843_pr3_claude_to_agy_next_turn() {
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/test_story_1843_pr3.py");
    let out = std::process::Command::new("python3")
        .arg(script)
        .args(["--aida", env!("CARGO_BIN_EXE_aida")])
        .output()
        .expect("run cross-vendor hook fixture");
    assert!(
        out.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
