#![cfg(target_os = "linux")]
//! The shell wrapper hands `aida init` a private side-channel file in
//! `AIDA_INIT_CD_FILE`. That path belongs to the ONE process the wrapper
//! started: the binary captures it at entry and removes it from its
//! environment, so nothing it spawns inherits it. A nested `aida init` (or any
//! child that calls `emit_enter_dir`) that saw the outer path would overwrite
//! the outer `cd` block with its own.
//!
//! Driven through the real binary: an extension subcommand (`aida-<name>` on
//! PATH) is a child `aida` spawns, and the stub behaves like a nested
//! emitter: it reports what it inherited and, if it got a path, writes its
//! own block there.
// trace:BUG-1960 | ai:claude

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

const NESTED_EMITTER_STUB: &str = r#"#!/bin/sh
printf 'inherited=[%s]\n' "${AIDA_INIT_CD_FILE-<unset>}"
if [ -n "${AIDA_INIT_CD_FILE-}" ]; then
    printf 'nested-block\n' > "$AIDA_INIT_CD_FILE"
fi
exit 0
"#;

#[test]
fn a_child_of_aida_never_inherits_or_overwrites_the_outer_side_channel_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    let home = tmp.path().join("home");
    let work = tmp.path().join("work");
    for d in [&bin_dir, &home, &work] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::create_dir_all(work.join(".git")).unwrap();
    let stub = bin_dir.join("aida-nestedemit");
    std::fs::write(&stub, NESTED_EMITTER_STUB).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let outer = tmp.path().join("aida-init-cd.outer");
    std::fs::write(&outer, "outer-block\n").unwrap();

    let path = format!("{}:/usr/bin:/bin", bin_dir.display());
    let mut out = None;
    // A stub written moments ago can still be held open by a sibling test's
    // fork; retry the rare "Text file busy" exec failure.
    for _ in 0..5 {
        let o = Command::new(env!("CARGO_BIN_EXE_aida"))
            .current_dir(&work)
            .env("HOME", &home)
            .env("PATH", &path)
            .env("AIDA_TELEMETRY", "0")
            .env("AIDA_INIT_CD_FILE", &outer)
            .env_remove("AIDA_BIN")
            .env_remove("AIDA_OUTPUT_FORMAT")
            .env_remove("AIDA_AGENT_OUTPUT")
            .arg("nestedemit")
            .output()
            .expect("run aida");
        if String::from_utf8_lossy(&o.stderr).contains("Text file busy") {
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        }
        out = Some(o);
        break;
    }
    let out = out.expect("aida ran");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the extension must run:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("inherited=[<unset>]"),
        "a child of aida must not inherit AIDA_INIT_CD_FILE:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(&outer).unwrap(),
        "outer-block\n",
        "a nested emitter must not overwrite the outer side-channel file"
    );
}
