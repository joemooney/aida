// trace:BUG-2752 | ai:codex
#[test]
fn bug_2752_codex_init_and_advisor_launch() {
    let result = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/codex_init.py"
        ))
        .arg(env!("CARGO_BIN_EXE_aida"))
        .status()
        .unwrap();
    assert!(result.success());
}
