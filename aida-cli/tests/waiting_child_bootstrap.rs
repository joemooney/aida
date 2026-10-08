//! Real normal-build CLI bootstrap, with no fixture decoder linked.
// trace:TASK-1612 | ai:codex
// trace:BUG-1808 | ai:codex
#![cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
use aida_core::launch_transport::{HostProfile, LaunchDescription, SealedExecImage, WaitingChild};
use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn digest(path: &Path, home: &Path) -> [u8; 32] {
    let output = Command::new("sha256sum")
        .arg(path)
        .env("HOME", home)
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let mut digest = [0; 32];
    for (i, b) in digest.iter_mut().enumerate() {
        *b = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap();
    }
    digest
}

#[cfg(target_os = "linux")]
fn ancestors() -> Vec<i32> {
    let mut ancestors = Vec::new();
    let mut pid = std::process::id() as i32;
    while pid > 1 {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let after = &stat[stat.rfind(')').unwrap() + 2..];
        pid = after.split(' ').nth(1).unwrap().parse().unwrap();
        ancestors.push(pid);
    }
    ancestors
}

#[test]
fn normal_cli_bootstrap_refuses_before_any_initialization() {
    for args in [
        vec!["--aida-waiting-child"],
        vec!["--aida-waiting-child", "release"],
    ] {
        let root = tempfile::tempdir().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_aida"))
            .args(args)
            .env_clear()
            .env("HOME", root.path())
            .current_dir(root.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(126));
        assert!(String::from_utf8_lossy(&output.stderr).contains("waiting-child refused"));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn normal_core_and_cli_can_only_prepare_observe_and_cancel() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let helper = root.path().join("helper");
    std::fs::copy(env!("CARGO_BIN_EXE_aida"), &helper).unwrap();
    assert!(Command::new("strip")
        .arg("--strip-all")
        .arg(&helper)
        .env("HOME", &home)
        .status()
        .unwrap()
        .success());
    let source = root.path().join("leaf.c");
    let leaf = root.path().join("leaf");
    std::fs::write(&source, "#include <stdio.h>\nint main(){FILE *f=fopen(\"must-not-exist\",\"w\");if(f)fclose(f);return 0;}\n").unwrap();
    assert!(Command::new("cc")
        .arg("-o")
        .arg(&leaf)
        .arg(&source)
        .env("HOME", &home)
        .status()
        .unwrap()
        .success());
    let helper = SealedExecImage::prepare(&helper, digest(&helper, &home)).unwrap();
    let leaf = SealedExecImage::prepare(&leaf, digest(&leaf, &home)).unwrap();
    let cwd = File::open(root.path()).unwrap();
    let null = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
        .unwrap();
    // trace:TASK-1612 | ai:claude
    // The public profile binds an observed adopter: an actual ancestor
    // (nearest subreaper) or namespace init, never a bare declaration.
    let profile = HostProfile::probe().unwrap();
    let ancestors = ancestors();
    assert!(
        ancestors.contains(&profile.adopter().pid),
        "{:?} not in {ancestors:?}",
        profile.adopter()
    );
    let mut child = WaitingChild::prepare(
        profile,
        helper,
        leaf,
        LaunchDescription {
            argv: vec!["compiled-fake".into()],
            environment: vec![("HOME".into(), home.to_str().unwrap().into())],
            startup_timeout: Duration::from_secs(30),
        },
        &cwd,
        [&null, &null, &null],
    )
    .unwrap();
    assert!(child.identity().start_ticks > 0);
    let canceled = child.cancel().unwrap();
    assert!(canceled.reaped && !canceled.possibly_executed);
    assert!(!root.path().join("must-not-exist").exists());
    assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0);
    assert!(!root.path().join(".aida").exists());
}
