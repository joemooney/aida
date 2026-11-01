//! Shared executable fixtures for tests. Creating the file, syncing it, and
//! closing its writer before marking it executable narrows the ETXTBSY window;
//! spawned fixture commands still use the production retry helper.
//! This is NOT only a test-harness concern. BUG-1735: the earlier claim here —
//! that production "invokes installed executables and its spawn sites already
//! retry ETXTBSY" — was false. A whole-crate survey found 4 of 161 non-literal
//! `Command::new` sites retrying, and the injectable ones (`resolve_gh_binary`
//! and its `glab` twin honour `AIDA_TEST_*_BINARY`; every `_using` seam takes a
//! path) sit in exactly the same window as a fixture. Spawn sites now route
//! through [`crate::process_retry::RetryEtxtbsy`].
// trace:BUG-1689 trace:BUG-1735 | ai:codex ai:claude

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

/// The CLI built alongside this test artifact, never an ambient installed CLI.
// trace:BUG-2398 | ai:codex
pub(crate) fn matching_aida_cli() -> std::path::PathBuf {
    let test = crate::aida_bin::current_executable_path().expect("test executable");
    let cli = test.parent().unwrap().parent().unwrap().join("aida");
    assert!(
        cli.is_file(),
        "build the matching CLI before these process fixtures: {}",
        cli.display()
    );
    cli
}

/// Fake Codex protocol for lifecycle fixtures: run the REAL matching CLI's
/// model-free authority probe, then leave normal model argv/body untouched.
/// This emulates only our emitted exact-key metadata overrides and core
/// filtering. Real policy/table/profile semantics belong to the native fixture.
// trace:BUG-2398 | ai:codex
pub(crate) fn codex_probe_script(body: impl AsRef<str>) -> String {
    let cli = matching_aida_cli();
    let keys = super::codex_tool_env::tool_env_keys_for_test();
    let script = format!(
        r#"#!/bin/sh
is_probe=false
for arg in "$@"; do
    [ "$arg" = sandbox ] && is_probe=true
done
if "$is_probe"; then
    python3 - "$@" <<'AIDA_CODEX_FIXTURE_PY'
import os, sys, tomllib
args = sys.argv[1:]
sep = args.index('--')
env = os.environ.copy()
for key in {keys:?}:
    env.pop(key, None)
i = 0
while i < sep:
    if args[i] == '-c':
        table = tomllib.loads(args[i+1])
        env.update(table.get('shell_environment_policy', {{}}).get('set', {{}}))
        i += 1
    i += 1
command = args[sep+1:]
assert command[1:3] == ['role', 'current'] and '--probe-seat' in command
os.execve({cli:?}, [{cli:?}] + command[1:], env)
AIDA_CODEX_FIXTURE_PY
    exit $?
fi
"#,
        cli = cli.to_string_lossy()
    );
    script
        + body
            .as_ref()
            .strip_prefix("#!/bin/sh\n")
            .unwrap_or(body.as_ref())
}

/// A synthetic reviewer worktree needs the same store link a real linked
/// worktree has. Point at the fixture's existing roster; never mint new
/// authority or change its seats to make the probe pass.
// trace:BUG-2398 | ai:codex
pub(crate) fn link_fixture_store(project: &std::path::Path, worktree: &std::path::Path) {
    if crate::gate::seat_authority::roster_store(worktree).is_ok() {
        return;
    }
    let store = crate::gate::seat_authority::roster_store(project).expect("fixture roster");
    crate::gate::seat_authority::test_support::assert_scratch_store(&store);
    std::fs::create_dir_all(worktree.join(".aida")).unwrap();
    let config = worktree.join(".aida/config.toml");
    assert!(
        !config.exists(),
        "do not overwrite an existing fixture configuration"
    );
    std::fs::write(
        config,
        format!(
            "store_path = {}\n",
            toml::Value::String(store.to_str().unwrap().into())
        ),
    )
    .unwrap();
}
