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
    // Keep discovery independent of companion binaries installed by CI (or
    // the host). Project discovery still needs Git; shell stubs use /bin/sh.
    // trace:STORY-2704 | ai:codex
    std::os::unix::fs::symlink("/usr/bin/git", bin_dir.join("git")).unwrap();
    // Startup prepends the running executable's directory to PATH. Execute a
    // fixture-owned hard link (or copy across filesystems), so companions in
    // Cargo's output directory cannot leak into discovery. A symlink would
    // canonicalize back to that shared directory.
    let aida_bin = bin_dir.join("aida");
    if std::fs::hard_link(env!("CARGO_BIN_EXE_aida"), &aida_bin).is_err() {
        std::fs::copy(env!("CARGO_BIN_EXE_aida"), &aida_bin).unwrap();
    }
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
    let path = fx.bin_dir.as_os_str();
    // A stub written moments ago can still be held open by a sibling test's
    // fork; retry the rare "Text file busy" exec failure.
    for attempt in 0..5 {
        let result = Command::new(fx.bin_dir.join("aida"))
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
            .output();
        // A sibling fork can briefly retain a just-copied fixture executable's
        // write handle too. Handle exec-time ETXTBSY as well as stub stderr.
        // trace:STORY-2704 | ai:codex
        let out = match result {
            Err(error) if error.raw_os_error() == Some(26) && attempt < 4 => {
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
            result => result.expect("run aida"),
        };
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
    let bin = canon(&fx.bin_dir.join("aida"));

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

// CI builds the companion in Cargo's directory. Prove the no-extension
// fixture actually executes elsewhere and keeps that companion out of PATH.
// trace:STORY-2704 | ai:codex
#[test]
fn fixture_executable_directory_excludes_built_companions() {
    let original = Path::new(env!("CARGO_BIN_EXE_aida"));
    assert!(
        original.with_file_name("aida-porcelain").is_file(),
        "build aida-porcelain beside aida first"
    );
    let fx = fixture(&[("aida-hello", RECORDING_STUB)]);
    assert_ne!(canon(&fx.bin_dir), canon(original.parent().unwrap()));
    assert!(!fx.bin_dir.join("aida-porcelain").exists());
    let out = aida(&fx, &fx.outside, &["hello"], &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        stdout(&out).contains(&format!("bin=[{}]", canon(&fx.bin_dir.join("aida")))),
        "{}",
        text(&out)
    );
    let empty = fixture(&[]);
    let out = aida(&empty, &empty.outside, &["--help"], &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        !stdout(&out).contains("External commands"),
        "{}",
        text(&out)
    );
}

// trace:STORY-1641 | ai:claude
#[test]
fn help_for_external_delegates_to_its_help() {
    let fx = fixture(&[("aida-hello", RECORDING_STUB)]);
    let out = aida(&fx, &fx.outside, &["help", "hello"], &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(stdout(&out).starts_with("arg=[--help]\n"), "{}", text(&out));
}

// The release companion is built beside aida by CI and the lane build job.
// Compare the external renderer with the retained substrate reader on the same
// empty-history fixture, including formatting and error channels.
// trace:STORY-2704 | ai:codex
#[test]
fn installed_flow_resolves_externally_with_reader_parity() {
    let fx = fixture(&[]);
    let companion = Path::new(env!("CARGO_BIN_EXE_aida")).with_file_name("aida-porcelain");
    assert!(
        companion.is_file(),
        "build aida-porcelain beside aida first"
    );
    std::fs::copy(companion, fx.bin_dir.join("aida-porcelain")).unwrap();
    std::os::unix::fs::symlink("aida-porcelain", fx.bin_dir.join("aida-flow")).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
        vec!["update-ref", "refs/remotes/origin/main", "HEAD"],
    ] {
        let result = Command::new("/usr/bin/git")
            .current_dir(&fx.proj)
            .args(args)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
    }
    for format in ["human", "toon", "json"] {
        for since in ["2000-01-01", "bad"] {
            let args = [
                "flow",
                "report",
                "--since",
                since,
                "--until",
                "2000-01-02",
                "--format",
                format,
            ];
            let external = aida(&fx, &fx.proj, &args, &[]);
            let mut reader_args = args;
            reader_args[0] = "flow-data";
            let reader = aida(&fx, &fx.proj, &reader_args, &[]);
            assert_eq!(
                external.status.code(),
                reader.status.code(),
                "{}",
                text(&external)
            );
            assert_eq!(
                external.stdout, reader.stdout,
                "format={format}, since={since}"
            );
            assert_eq!(
                external.stderr, reader.stderr,
                "format={format}, since={since}"
            );
            if since != "bad" {
                assert!(external.status.success(), "{}", text(&external));
            }
        }
    }
}

// Exercise upgrade_cmd's download/extract/install path, not just the shared
// installer script. A fake curl supplies a real local release tarball.
// trace:STORY-2704 | ai:codex
#[test]
fn tarball_upgrade_replaces_pair_companion_and_link() {
    let fx = fixture(&[]);
    for utility in [
        "tar", "gzip", "sh", "install", "mktemp", "mkdir", "cp", "mv", "rm", "ln",
    ] {
        std::os::unix::fs::symlink(
            Path::new("/usr/bin").join(utility),
            fx.bin_dir.join(utility),
        )
        .unwrap();
    }
    let source = fx._tmp.path().join("bundle");
    let installed = fx._tmp.path().join("installed");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&installed).unwrap();
    for name in ["aida", "aida-server", "aida-porcelain"] {
        write_exec(&source.join(name), &format!("#!/bin/sh\necho new-{name}\n"));
        write_exec(&installed.join(name), "#!/bin/sh\necho 'aida 0.0.1'\n");
    }
    std::os::unix::fs::symlink("aida-porcelain", source.join("aida-flow")).unwrap();
    write_exec(&installed.join("aida-flow"), "#!/bin/sh\necho old-flow\n");
    let archive = fx._tmp.path().join("release.tar.gz");
    assert!(Command::new("/usr/bin/tar")
        .arg("czf")
        .arg(&archive)
        .arg("-C")
        .arg(&source)
        .args(["aida", "aida-server", "aida-porcelain", "aida-flow"])
        .status()
        .unwrap()
        .success());
    write_exec(
        &fx.bin_dir.join("curl"),
        "#!/bin/sh\n[ \"$1 $2\" = '-fSL -o' ] || exit 19\ncp \"$FIXTURE_ARCHIVE\" \"$3\"\n",
    );
    let output = aida(
        &fx,
        &fx.outside,
        &[
            "upgrade",
            "--target",
            installed.join("aida").to_str().unwrap(),
            "--version",
            "v99.0.0",
            "--yes",
        ],
        &[("FIXTURE_ARCHIVE", archive.to_str().unwrap())],
    );
    assert!(output.status.success(), "{}", text(&output));
    for name in ["aida", "aida-server", "aida-porcelain", "aida-flow"] {
        let label = if name == "aida-flow" {
            "aida-porcelain"
        } else {
            name
        };
        let output = Command::new(installed.join(name)).output().unwrap();
        assert_eq!(output.stdout, format!("new-{label}\n").as_bytes());
    }
    assert_eq!(
        std::fs::read_link(installed.join("aida-flow")).unwrap(),
        Path::new("aida-porcelain")
    );
}

// trace:STORY-2704 | ai:codex
#[test]
fn nested_external_dispatch_help_and_builtin_precedence() {
    let fx = fixture(&[
        ("aida-review-summary", RECORDING_STUB),
        ("aida-review-record", MARKER_STUB),
        ("aida-queue-summary", MARKER_STUB),
        ("aida-agent-summary", MARKER_STUB),
        ("aida-show-summary", MARKER_STUB),
        ("aida-flow", RECORDING_STUB),
        ("aida-flow-report", MARKER_STUB),
    ]);
    let out = aida(
        &fx,
        &fx.outside,
        &["--format", "json", "review", "summary", "a b", "--flag"],
        &[],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        stdout(&out).contains("arg=[a b]\narg=[--flag]"),
        "{}",
        text(&out)
    );
    assert!(stdout(&out).contains("format=[json]"), "{}", text(&out));
    let out = aida(
        &fx,
        &fx.outside,
        &[
            "review",
            "--target-branch",
            "summary",
            "summary",
            "--",
            "--flag",
        ],
        &[],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        stdout(&out).starts_with("arg=[--]\narg=[--flag]\n"),
        "{}",
        text(&out)
    );
    let out = aida(
        &fx,
        &fx.outside,
        &["review", "summary", "--format", "extension-value"],
        &[],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        stdout(&out).starts_with("arg=[--format]\narg=[extension-value]\n"),
        "{}",
        text(&out)
    );
    for args in [
        &["review", "summary", "--help"][..],
        &["help", "review", "summary"][..],
    ] {
        let out = aida(&fx, &fx.outside, args, &[]);
        assert!(out.status.success(), "{}", text(&out));
        assert!(stdout(&out).starts_with("arg=[--help]\n"), "{}", text(&out));
    }
    for args in [&["review", "--help"][..], &["help", "review"][..]] {
        let out = aida(&fx, &fx.outside, args, &[]);
        assert!(out.status.success(), "{}", text(&out));
        assert!(stdout(&out).contains("External commands"), "{}", text(&out));
        assert!(
            stdout(&out).contains(&fx.bin_dir.join("aida-review-summary").display().to_string()),
            "{}",
            text(&out)
        );
        assert!(
            !stdout(&out).contains("aida-review-record"),
            "{}",
            text(&out)
        );
    }
    for parent in ["queue", "agent"] {
        let out = aida(&fx, &fx.outside, &[parent, "--help"], &[]);
        assert!(out.status.success(), "{}", text(&out));
        assert!(
            !stdout(&out).contains(&format!("aida-{parent}-summary")),
            "{}",
            text(&out)
        );
    }
    let out = aida(&fx, &fx.outside, &["alias", "list", "--json"], &[]);
    assert!(
        stdout(&out).contains("aida review summary"),
        "{}",
        text(&out)
    );
    assert!(
        !stdout(&out).contains("aida-review-record"),
        "{}",
        text(&out)
    );
    // Known-child errors, invalid options, missing option values, positional
    // operands and path-like words must never enter an extension.
    for args in [
        &["review", "record", "--invalid"][..],
        &["review", "--invalid", "summary"][..],
        &["--format", "review", "summary"][..],
        &["review", "../summary"][..],
        &["review", "--", "summary"][..],
        &["review", "TASK-1", "summary"][..],
        &["show", "summary"][..],
    ] {
        let out = aida(&fx, &fx.outside, args, &[]);
        assert!(!out.status.success(), "{}", text(&out));
        assert!(!text(&out).contains("STUB-RAN"), "{}", text(&out));
        assert!(!stdout(&out).contains("arg="), "{}", text(&out));
    }
    // An external parent owns the remainder, without another lookup.
    let out = aida(&fx, &fx.outside, &["flow", "report"], &[]);
    assert!(stdout(&out).starts_with("arg=[report]\n"), "{}", text(&out));
}

// trace:STORY-2704 | ai:codex
#[test]
fn nested_missing_extension_preserves_clap_diagnostic() {
    let fx = fixture(&[]);
    let original = aida(&fx, &fx.outside, &["config", "nosuch"], &[]);
    write_exec(&fx.bin_dir.join("aida-review-summary"), RECORDING_STUB);
    let installed = aida(&fx, &fx.outside, &["config", "nosuch"], &[]);
    assert_eq!(original.status.code(), Some(2), "{}", text(&original));
    assert_eq!(original.status.code(), installed.status.code());
    assert_eq!(original.stderr, installed.stderr);
    assert!(String::from_utf8_lossy(&original.stderr).contains("unrecognized subcommand"));
}
