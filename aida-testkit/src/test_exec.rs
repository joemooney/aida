//! Shared executable fixtures for tests. Creating the file, syncing it, and
//! closing its writer before marking it executable narrows the ETXTBSY window;
//! spawned fixture commands use the same bounded ETXTBSY retry policy.
//! This is NOT only a test-harness concern. BUG-1735: the earlier claim here —
//! that production "invokes installed executables and its spawn sites already
//! retry ETXTBSY" — was false. A whole-crate survey found 4 of 161 non-literal
//! `Command::new` sites retrying, and the injectable ones (`resolve_gh_binary`
//! and its `glab` twin honour `AIDA_TEST_*_BINARY`; every `_using` seam takes a
//! path) sit in exactly the same window as a fixture. Spawn sites now route
//! through `aida_term::process_retry::RetryEtxtbsy`.
// trace:BUG-1689 trace:BUG-1735 | ai:codex ai:claude

// trace:TASK-3784 | ai:codex

use std::io::Write;

pub fn write_executable(path: &std::path::Path, body: impl AsRef<str>) {
    let mut file = std::fs::File::create(path).expect("create executable fixture");
    file.write_all(body.as_ref().as_bytes())
        .expect("write executable fixture");
    file.sync_all().expect("sync executable fixture");
    drop(file);

    mark_executable(path);
}

pub fn mark_executable(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)
            .expect("stat executable fixture")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("chmod executable fixture");
    }
}

pub fn output(command: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    for attempt in 0..=5 {
        match command.output() {
            #[cfg(unix)]
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && attempt < 5 => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            result => return result,
        }
    }
    unreachable!("bounded retry loop always returns")
}

#[cfg(target_os = "linux")]
#[test]
fn fixture_spawn_retries_while_writer_descriptor_is_open() {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;

    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("fixture");
    write_executable(&executable, "#!/bin/sh\necho ready\n");

    // Linux rejects exec with ETXTBSY while a writer descriptor is open.
    let mut writer = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_CLOEXEC)
        .open(&executable)
        .unwrap();
    writer.write_all(b"#!/bin/sh\necho ready\n").unwrap();
    writer.sync_all().unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(80));
        drop(writer);
    });

    let output = output(&mut std::process::Command::new(&executable)).unwrap();
    release.join().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ready");
}

/// Return the profile directory for either Cargo's legacy deps runner or its
/// unit build directory. Match the full suffix rather than an arbitrary ancestor
/// named `build`, since target directories and profiles may have custom names.
// trace:BUG-2539 | ai:codex
pub fn test_profile_dir(executable: &std::path::Path) -> Option<&std::path::Path> {
    let directory = executable.parent()?;
    if directory.file_name()? == "deps" {
        return directory.parent();
    }
    if directory.file_name()? == "out" {
        let unit = directory.parent()?;
        let package = unit.parent()?;
        let build = package.parent()?;
        if build.file_name()? == "build" {
            return build.parent();
        }
    }
    None
}

/// Re-exec the running test image, independent of ambient CLI overrides.
// trace:BUG-2539 | ai:codex
pub fn current_test_runner() -> std::path::PathBuf {
    let runner = std::env::current_exe().expect("test executable");
    assert!(
        test_profile_dir(&runner).is_some(),
        "unrecognized Cargo test executable layout: {}",
        runner.display()
    );
    runner
}

// trace:BUG-2539 | ai:codex
#[test]
fn bug_2539_test_profile_recognizes_both_cargo_layouts() {
    use std::path::Path;
    for profile in ["debug", "release", "agent", "custom-profile"] {
        let root = std::path::PathBuf::from("/custom/build/target").join(profile);
        for suffix in [
            "deps/runner-123",
            "build/aida-cli-lib/012345/out/runner-123",
        ] {
            assert_eq!(test_profile_dir(&root.join(suffix)), Some(root.as_path()));
        }
    }
    for path in [
        "/target/debug/aida",
        "/target/debug/build/pkg/hash/runner",
        "/target/debug/other/pkg/hash/out/runner",
        "/ambient/aida",
    ] {
        assert_eq!(test_profile_dir(Path::new(path)), None, "{path}");
    }
}

/// The CLI built alongside this test artifact, never an ambient installed CLI.
// trace:BUG-2398 trace:BUG-2539 | ai:codex
pub fn matching_aida_cli() -> std::path::PathBuf {
    let test = current_test_runner();
    let cli = test_profile_dir(&test).unwrap().join("aida");
    assert!(
        cli.is_file(),
        "build the matching CLI before these process fixtures: {}",
        cli.display()
    );
    cli
}
