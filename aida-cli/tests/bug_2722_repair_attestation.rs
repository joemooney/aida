// trace:BUG-2722 | ai:codex
#[test]
fn bug_2722_real_cli_terminal_attestation() {
    let result = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/repair_attestation.py"
        ))
        .env("AIDA_BIN", env!("CARGO_BIN_EXE_aida"))
        .status()
        .unwrap();
    assert!(result.success());
}
