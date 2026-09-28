//! Shared executable fixtures for tests. Creating the file, syncing it, and
//! closing its writer before marking it executable narrows the ETXTBSY window;
//! spawned fixture commands still use the production retry helper.
//! This is a test-harness concern: production invokes installed executables
//! and its spawn sites already retry ETXTBSY.
// trace:BUG-1689 | ai:codex

use std::io::Write;

pub(crate) fn write_executable(path: &std::path::Path, body: impl AsRef<str>) {
    let mut file = std::fs::File::create(path).expect("create executable fixture");
    file.write_all(body.as_ref().as_bytes())
        .expect("write executable fixture");
    file.sync_all().expect("sync executable fixture");
    drop(file);

    mark_executable(path);
}

pub(crate) fn mark_executable(path: &std::path::Path) {
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

pub(crate) fn output(command: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    crate::process_retry::command_output_retrying_etxtbsy(command)
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
