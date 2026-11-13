//! Host-only split merge command fixtures.
// trace:STORY-2692 | ai:codex
#[test]
fn split_merge_host_fixtures() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let output = std::process::Command::new("python3")
        .arg(root.join("tests/test_port_split_hunks.py"))
        .current_dir(root)
        .output()
        .expect("python3 fixture runner");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
