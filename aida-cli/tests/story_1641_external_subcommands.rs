#![cfg(target_os = "linux")]
//! Black-box coverage for git-style external subcommands: an unrecognised
//! `aida <name>` runs an executable `aida-<name>` from PATH, built-ins and
//! (human) user aliases always win, the extension sees its context in the
//! environment, and discovery lists installed extensions.
// trace:STORY-1641 | ai:claude

use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Prints its argv and the context variables, one per line, then exits as
/// asked by its first argument.
const RECORDING_STUB: &str = r#"#!/bin/sh
for a in "$@"; do printf 'arg=[%s]\n' "$a"; done
printf 'root=[%s]\n' "${AIDA_PROJECT_ROOT-<unset>}"
printf 'store=[%s]\n' "${AIDA_STORE_PATH-<unset>}"
printf 'format=[%s]\n' "${AIDA_OUTPUT_FORMAT-<unset>}"
printf 'kind=[%s]\n' "${AIDA_CALLER_KIND-<unset>}"
printf 'bin=[%s]\n' "${AIDA_BIN-<unset>}"
echo "stub-stderr" >&2
case "$1" in
  exit3) exit 3 ;;
  term) kill -TERM $$ ;;
esac
exit 0
"#;

const MARKER_STUB: &str = "#!/bin/sh\necho STUB-RAN\necho STUB-RAN >&2\nexit 0\n";

struct Fx {
    _tmp: tempfile::TempDir,
    bin_dir: PathBuf,
    home: PathBuf,
    proj: PathBuf,
    outside: PathBuf,
}

fn write_exec(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

/// A fixture whose PATH directory holds `stubs` (`(file name, body)`), an
/// AIDA project (`proj`, with a store) and a plain directory outside any
/// project (`outside`, its own git checkout so no ancestor is consulted).
fn fixture(stubs: &[(&str, &str)]) -> Fx {
    let tmp = tempfile::TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    let outside = tmp.path().join("outside");
    for d in [&bin_dir, &home, &proj, &outside] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::create_dir_all(proj.join(".aida")).unwrap();
    std::fs::create_dir_all(proj.join(".aida-store")).unwrap();
    std::fs::create_dir_all(proj.join("sub").join("deeper")).unwrap();
    std::fs::write(
        proj.join(".aida").join("config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(outside.join(".git")).unwrap();
    for (name, body) in stubs {
        write_exec(&bin_dir.join(name), body);
    }
    Fx {
        _tmp: tmp,
        bin_dir,
        home,
        proj,
        outside,
    }
}

fn aida(fx: &Fx, dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let path = format!("{}:/usr/bin:/bin", fx.bin_dir.display());
    // A stub written moments ago can still be held open by a sibling test's
    // fork; retry the rare "Text file busy" exec failure.
    for attempt in 0..5 {
        let out = Command::new(env!("CARGO_BIN_EXE_aida"))
            .current_dir(dir)
            .env("HOME", &fx.home)
            .env("PATH", &path)
            .env("AIDA_TELEMETRY", "0")
            .env_remove("AIDA_OUTPUT_FORMAT")
            .env_remove("AIDA_AGENT_OUTPUT")
            .env_remove("AIDA_AGENT_TYPE")
            .env_remove("AIDA_HEADLESS")
            .env_remove("AIDA_STORE")
            .env_remove("AIDA_STORE_PATH")
            .env_remove("AIDA_PROJECT_ROOT")
            .env_remove("AIDA_BIN")
            .env_remove("AIDA_CALLER_KIND")
            .envs(env.iter().copied())
            .args(args)
            .output()
            .expect("run aida");
        let busy = String::from_utf8_lossy(&out.stderr).contains("Text file busy");
        if busy && attempt < 4 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        }
        return out;
    }
    unreachable!()
}

fn text(out: &Output) -> String {
    format!(
        "status: {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn canon(p: &Path) -> String {
    std::fs::canonicalize(p).unwrap().display().to_string()
}

// trace:STORY-1641 | ai:claude
#[test]
fn external_subcommand_execs_aida_prefixed_binary() {
    let fx = fixture(&[("aida-hello", RECORDING_STUB)]);

    let out = aida(&fx, &fx.outside, &["hello", "a", "b c"], &[]);
    assert!(out.status.success(), "{}", text(&out));
    let so = stdout(&out);
    assert!(
        so.starts_with("arg=[a]\narg=[b c]\nroot="),
        "{}",
        text(&out)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "stub-stderr\n",
        "{}",
        text(&out)
    );

    let out = aida(&fx, &fx.outside, &["hello", "exit3"], &[]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));

    let out = aida(&fx, &fx.outside, &["hello", "term"], &[]);
    assert_eq!(out.status.signal(), Some(15), "{}", text(&out));
}

// trace:STORY-1641 | ai:claude
#[test]
fn builtin_shadows_external() {
    let fx = fixture(&[("aida-list", MARKER_STUB)]);
    let out = aida(&fx, &fx.outside, &["list"], &[]);
    assert!(!text(&out).contains("STUB-RAN"), "{}", text(&out));

    let help = aida(&fx, &fx.outside, &["help"], &[]);
    assert!(
        !stdout(&help).contains("External commands"),
        "{}",
        text(&help)
    );
}

/// Agents never get user aliases expanded, so a same-named extension is what
/// they reach — and they may run extensions at all.
// trace:STORY-1641 | ai:claude
#[test]
fn agent_caller_runs_external_even_when_user_alias_exists() {
    let fx = fixture(&[("aida-foo", RECORDING_STUB)]);
    std::fs::create_dir_all(fx.home.join(".aida")).unwrap();
    std::fs::write(
        fx.home.join(".aida").join("aliases.toml"),
        "[alias]\nfoo = \"list\"\n",
    )
    .unwrap();
    let out = aida(
        &fx,
        &fx.outside,
        &["foo", "x"],
        &[("AIDA_AGENT_TYPE", "codex")],
    );
    assert!(out.status.success(), "{}", text(&out));
    let so = stdout(&out);
    assert!(so.contains("arg=[x]\n"), "{}", text(&out));
    assert!(so.contains("kind=[agent]\n"), "{}", text(&out));
}

// trace:STORY-1641 | ai:claude
#[test]
fn external_receives_context_env() {
    let fx = fixture(&[("aida-hello", RECORDING_STUB)]);
    let root = canon(&fx.proj);
    let store = format!("{root}/.aida-store");
    let bin = canon(Path::new(env!("CARGO_BIN_EXE_aida")));

    for dir in [fx.proj.clone(), fx.proj.join("sub").join("deeper")] {
        let out = aida(&fx, &dir, &["hello"], &[]);
        assert!(out.status.success(), "{}", text(&out));
        let so = stdout(&out);
        assert!(so.contains(&format!("root=[{root}]\n")), "{}", text(&out));
        assert!(so.contains(&format!("store=[{store}]\n")), "{}", text(&out));
        assert!(so.contains("format=[toon]\n"), "{}", text(&out));
        assert!(so.contains("kind=[agent]\n"), "{}", text(&out));
        assert!(so.contains(&format!("bin=[{bin}]\n")), "{}", text(&out));
    }

    // Outside any AIDA project the project variables are unset, even when the
    // caller's own environment carried stale values.
    let out = aida(
        &fx,
        &fx.outside,
        &["hello"],
        &[
            ("AIDA_PROJECT_ROOT", "/stale/root"),
            ("AIDA_STORE_PATH", "/stale/store"),
            ("AIDA_OUTPUT_FORMAT", "json"),
            ("AIDA_HEADLESS", "1"),
        ],
    );
    assert!(out.status.success(), "{}", text(&out));
    let so = stdout(&out);
    assert!(so.contains("root=[<unset>]\n"), "{}", text(&out));
    assert!(so.contains("store=[<unset>]\n"), "{}", text(&out));
    assert!(so.contains("format=[json]\n"), "{}", text(&out));
    assert!(so.contains("kind=[headless]\n"), "{}", text(&out));
}

// trace:STORY-1641 | ai:claude
#[test]
fn unknown_name_without_extension_keeps_error_and_exit_code() {
    let fx = fixture(&[]);
    let out = aida(&fx, &fx.outside, &["nosuch"], &[]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unrecognized subcommand"), "{}", text(&out));
    assert!(
        !err.contains("hint: an installed extension"),
        "{}",
        text(&out)
    );

    // A near-miss of an installed extension adds a hint, same error and code.
    let fx = fixture(&[("aida-hallo", RECORDING_STUB)]);
    let out = aida(&fx, &fx.outside, &["hello"], &[]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unrecognized subcommand"), "{}", text(&out));
    assert!(err.contains("`aida hallo`"), "{}", text(&out));
    assert!(!stdout(&out).contains("arg="), "{}", text(&out));
}

// trace:STORY-1641 | ai:claude
#[test]
fn help_and_alias_list_show_externals() {
    let fx = fixture(&[("aida-hello", RECORDING_STUB)]);
    let path = fx.bin_dir.join("aida-hello").display().to_string();

    for args in [&["help"][..], &["help", "--all"][..]] {
        let out = aida(&fx, &fx.outside, args, &[]);
        assert!(out.status.success(), "{}", text(&out));
        let so = stdout(&out);
        assert!(so.contains("External commands"), "{}", text(&out));
        assert!(so.contains("hello"), "{}", text(&out));
        assert!(so.contains(&path), "{}", text(&out));
    }

    for args in [
        &["alias", "list"][..],
        &["--format", "toon", "alias", "list"][..],
    ] {
        let out = aida(&fx, &fx.outside, args, &[]);
        assert!(out.status.success(), "{}", text(&out));
        let so = stdout(&out);
        assert!(so.contains("External commands"), "{}", text(&out));
        assert!(so.contains("aida hello"), "{}", text(&out));
        assert!(so.contains(&path), "{}", text(&out));
    }

    let out = aida(&fx, &fx.outside, &["alias", "list", "--json"], &[]);
    assert!(out.status.success(), "{}", text(&out));
    let groups: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let external = groups
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["surface"] == "External commands")
        .unwrap_or_else(|| panic!("no external group: {}", text(&out)));
    let row = &external["rows"][0];
    assert_eq!(row["alias"], "aida hello");
    assert_eq!(row["expands_to"], path.as_str());
    assert!(row["meaning"].as_str().unwrap().contains("external"));

    // Nothing extra when no extension is installed.
    let none = fixture(&[]);
    for args in [
        &["help"][..],
        &["alias", "list"][..],
        &["alias", "list", "--json"][..],
    ] {
        let out = aida(&none, &none.outside, args, &[]);
        assert!(out.status.success(), "{}", text(&out));
        assert!(
            !stdout(&out).contains("External commands"),
            "{}",
            text(&out)
        );
    }
}

// trace:STORY-1641 | ai:claude
#[test]
fn help_for_external_delegates_to_its_help() {
    let fx = fixture(&[("aida-hello", RECORDING_STUB)]);
    let out = aida(&fx, &fx.outside, &["help", "hello"], &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(stdout(&out).starts_with("arg=[--help]\n"), "{}", text(&out));
}
