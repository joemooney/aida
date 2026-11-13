//! Built binary parity, with no installed AIDA dependency. trace:STORY-2704 | ai:codex
#[test]
fn core_external_porcelain_parity() {
    let builtin = std::path::Path::new(env!("CARGO_BIN_EXE_aida"));
    let porcelain = builtin.with_file_name("aida-porcelain");
    assert!(
        porcelain.is_file(),
        "build aida-porcelain alongside aida before this test"
    );
    let result = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/test_porcelain_flow_parity.py"
        ))
        .arg(builtin)
        .arg(porcelain)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
