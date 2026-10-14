//! BUG-0445: the `aida()` wrapper's `init` arm must run the binary with its
//! stdout ATTACHED, and receive the `cd` block through a side-channel file.
//!
//! The arm used to capture stdout (`_aida_out=$(command aida "$@" ...)`) to
//! slice the marker-delimited `cd` block out of it. The child therefore saw a
//! pipe: it auto-selected the agent (TOON) format, and when human output was
//! pinned every prompt and progress line was buffered inside the substitution
//! until exit, so a prompt waiting on terminal stdin looked like a hang.
//!
//! The fix hands the binary a private file in `AIDA_INIT_CD_FILE`; the binary
//! writes the SAME marker-delimited block there, and the wrapper evals only a
//! file holding exactly one marker pair. These tests drive the real wrapper
//! through the real shell matrix (`wrapper_shells()`: bash, plus zsh when the
//! runner has it) with a stub `aida` on PATH, and — where util-linux `script`
//! is available — through a real pseudo-terminal, so the child's `[ -t 1 ]`
//! verdict is observed rather than inferred.
#![cfg(unix)]
// trace:BUG-0445 | ai:claude

use crate::process_retry::RetryEtxtbsy;
use crate::shell_eval::{EVAL_BEGIN, EVAL_END, INIT_CD_FILE_ENV};
use crate::shell_wrapper_harness::{run_wrapper_in, wrapper_shells};
use std::sync::OnceLock;

// ── stubs ────────────────────────────────────────────────────────────────────

/// A stub `aida init <DIR>` in the shape of the fixed binary: it records what
/// its own fd 1 looks like (BEFORE any redirection, so the verdict is about the
/// stdout the wrapper handed it), prints prose, creates the directory, and
/// writes the marked `cd` block to the side-channel file. `exit_code` lets the
/// failure path reuse the same shape.
// trace:BUG-0445 | ai:claude
fn side_channel_stub(exit_code: i32) -> String {
    format!(
        "#!/usr/bin/env bash\n\
         if [ -t 1 ]; then _t=tty; else _t=notty; fi\n\
         if [ -p /dev/stdout ]; then _p=pipe; else _p=notpipe; fi\n\
         printf '%s %s\\n' \"$_t\" \"$_p\" > \"$AIDA_T_PROBE\"\n\
         printf '%s\\n' \"${{{env}-unset}}\" > \"$AIDA_T_FILE\"\n\
         printf 'Initializing %s\\n' \"$2\"\n\
         mkdir -p \"$2\"\n\
         [ {exit_code} -eq 0 ] || printf 'error: forge refused\\n' >&2\n\
         printf '%s\\n' '{begin}' \"cd '$PWD/$2'\" '{end}' > \"${env}\"\n\
         exit {exit_code}\n",
        env = INIT_CD_FILE_ENV,
        begin = EVAL_BEGIN,
        end = EVAL_END,
    )
}

/// A stub that writes `content_lines` (shell `printf '%s\n'` arguments) to the
/// side-channel file: the malformed-file cases.
// trace:BUG-0445 | ai:claude
fn file_content_stub(content_lines: &str) -> String {
    format!(
        "#!/usr/bin/env bash\n\
         mkdir -p \"$2\"\n\
         printf '%s\\n' {content_lines} > \"${env}\"\n\
         exit 0\n",
        env = INIT_CD_FILE_ENV,
    )
}

/// A binary from before the side channel: it never touches the file. This is
/// the shape of an older binary that saw the (no longer advertised) `init-cd`
/// token — markers on stdout — and of a bare in-place init that prints prose.
/// The `cd` is relative so the printed block is identical across the per-shell
/// tempdirs `init_side_channel_agrees_across_shells` compares byte for byte.
// trace:BUG-0445 | ai:claude
// trace:BUG-1960 | ai:claude
fn stdout_only_stub() -> String {
    format!(
        "#!/usr/bin/env bash\n\
         mkdir -p \"$2\"\n\
         printf 'project ready:\\n'\n\
         printf '%s\\n' '{begin}' \"cd '$2'\" '{end}'\n\
         exit 0\n",
        begin = EVAL_BEGIN,
        end = EVAL_END,
    )
}

/// The body every scenario runs: export the probe paths the stub writes to,
/// invoke the wrapper, then report status, where the shell ended up, whether
/// the per-command env var leaked, and whether the side-channel file is gone.
// trace:BUG-0445 | ai:claude
fn init_body(dir: &str) -> String {
    format!(
        "export AIDA_T_PROBE=\"$PWD/probe\" AIDA_T_FILE=\"$PWD/cdfile\"\n\
         _start=$PWD\n\
         aida init {dir}\n\
         echo \"rc:$?\"\n\
         if [ \"$PWD\" = \"$_start/{dir}\" ]; then echo moved:yes; else echo moved:no; fi\n\
         echo \"leak:${{{env}-unset}}\"\n\
         _f=$(cat \"$_start/cdfile\" 2>/dev/null)\n\
         if [ -n \"$_f\" ] && [ -e \"$_f\" ]; then echo file:present; else echo file:gone; fi\n\
         echo \"probe:$(cat \"$_start/probe\" 2>/dev/null)\"\n",
        env = INIT_CD_FILE_ENV,
    )
}

// ── the regression: the child's stdout is not captured ──────────────────────

/// With the wrapper's own stdout being a pipe (the test harness captures it),
/// the child must see THAT pipe — not a second one opened by a command
/// substitution. We cannot tell two pipes apart from inside the child, so the
/// companion test below redirects the wrapper's stdout to a regular file and
/// demands the child see a file, which a capturing wrapper can never satisfy.
/// This one pins the observable half: the prose reaches the harness, the `cd`
/// still applies, and the markers never appear on stdout.
// trace:BUG-0445 | ai:claude
#[test]
fn init_prose_is_displayed_and_the_side_channel_cd_applies() {
    for &shell in wrapper_shells() {
        let (stdout, stderr, _) = run_wrapper_in(shell, &side_channel_stub(0), &init_body("proj"));
        assert!(
            stdout.contains("Initializing proj"),
            "[{shell}] the child's prose must reach stdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(stdout.contains("rc:0"), "[{shell}] {stdout}");
        assert!(
            stdout.contains("moved:yes"),
            "[{shell}] the calling shell must end up inside the new project:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            stdout.contains("leak:unset"),
            "[{shell}] the per-command env var must not leak into the shell:\n{stdout}"
        );
        assert!(
            stdout.contains("file:gone"),
            "[{shell}] the side-channel file must be removed:\n{stdout}"
        );
        assert!(
            !stdout.contains(EVAL_BEGIN) && !stdout.contains(EVAL_END),
            "[{shell}] markers travel on the side channel, never stdout:\n{stdout}"
        );
        assert!(
            !stderr.contains("command not found"),
            "[{shell}] nothing on stdout may be eval'd:\n{stderr}"
        );
    }
}

/// The structural regression test. Redirect the WRAPPER's stdout to a regular
/// file; the child must then see a regular file on its fd 1 too. A wrapper
/// that captures the child in `$(...)` hands it a pipe instead, which is
/// exactly the condition that made the binary auto-select the agent format
/// and buffer its prompts.
// trace:BUG-0445 | ai:claude
#[test]
fn init_child_inherits_the_wrappers_stdout_not_a_capture_pipe() {
    for &shell in wrapper_shells() {
        let body = "export AIDA_T_PROBE=\"$PWD/probe\" AIDA_T_FILE=\"$PWD/cdfile\"\n\
                    _start=$PWD\n\
                    aida init proj > \"$_start/wrapper-stdout\"\n\
                    echo \"rc:$?\"\n\
                    echo \"probe:$(cat \"$_start/probe\")\"\n\
                    cat \"$_start/wrapper-stdout\"\n";
        let (stdout, stderr, _) = run_wrapper_in(shell, &side_channel_stub(0), body);
        assert!(stdout.contains("rc:0"), "[{shell}] {stdout}\n{stderr}");
        assert!(
            stdout.contains("probe:notty notpipe"),
            "[{shell}] the child must see the wrapper's own stdout (a file here), \
             not a capture pipe:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            stdout.contains("Initializing proj"),
            "[{shell}] the child's prose went to the wrapper's stdout:\n{stdout}"
        );
    }
}

/// When the CALLER pipes the wrapper (`aida init x | cat`), the child must see
/// that pipe — non-interactive format selection is the caller's choice and the
/// wrapper must neither hide nor add to it.
// trace:BUG-0445 | ai:claude
#[test]
fn a_caller_pipe_is_still_a_pipe_for_the_child() {
    for &shell in wrapper_shells() {
        let body = "export AIDA_T_PROBE=\"$PWD/probe\" AIDA_T_FILE=\"$PWD/cdfile\"\n\
                    _start=$PWD\n\
                    aida init proj | cat\n\
                    echo \"probe:$(cat \"$_start/probe\")\"\n";
        let (stdout, stderr, _) = run_wrapper_in(shell, &side_channel_stub(0), body);
        assert!(
            stdout.contains("probe:notty pipe"),
            "[{shell}] a caller-side pipe must reach the child unchanged:\n{stdout}\nstderr:\n{stderr}"
        );
    }
}

// ── a real terminal ──────────────────────────────────────────────────────────

/// util-linux `script`, when it runs here: it is the only portable-enough way
/// to give the wrapper a real pseudo-terminal from a Rust test. BSD `script`
/// has a different flag set and no `--version`, so it is a clean skip.
// trace:BUG-0445 | ai:claude
fn util_linux_script_runs() -> bool {
    static PROBE: OnceLock<bool> = OnceLock::new();
    *PROBE.get_or_init(|| {
        let ok = std::process::Command::new("script")
            .arg("--version")
            .output_retrying_etxtbsy()
            .map(|out| {
                out.status.success() && String::from_utf8_lossy(&out.stdout).contains("util-linux")
            })
            .unwrap_or(false);
        if !ok {
            eprintln!(
                "note: util-linux `script` not available — the real-PTY init test is skipped \
                 on this machine."
            );
        }
        ok
    })
}

/// Like `run_wrapper_in`, but the shell runs INSIDE a pseudo-terminal created
/// by `script`, so `[ -t 1 ]` inside the stub is a real verdict. Returns the
/// PTY transcript (CRLF line endings) and the shell's exit status.
// trace:BUG-0445 | ai:claude
fn run_wrapper_in_pty(shell: &str, stub: &str, body: &str) -> (String, Option<i32>) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let bin = dir.path().join("aida");
    crate::test_exec::write_executable(&bin, stub);
    let script_path = dir.path().join("session.sh");
    std::fs::write(
        &script_path,
        format!(
            "PATH='{path}':\"$PATH\"\nexport PATH\n{helpers}\n{body}\n",
            path = dir.path().display(),
            helpers = crate::dev_cmd::SHELL_HELPERS,
            body = body,
        ),
    )
    .unwrap();
    let shell_cmd = match shell {
        "zsh" => format!("zsh -f {}", script_path.display()),
        _ => format!("bash {}", script_path.display()),
    };
    let out = std::process::Command::new("script")
        .args(["-q", "-e", "-c", shell_cmd.as_str(), "/dev/null"])
        .env_clear()
        .env("HOME", &home)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(dir.path())
        .stdin(std::process::Stdio::null())
        .output_retrying_etxtbsy()
        .unwrap_or_else(|e| panic!("script available: {e}"));
    (
        String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"),
        out.status.code(),
    )
}

/// The acceptance case in its literal form: on a real terminal, the wrapped
/// `aida init <DIR>` child sees a TTY on stdout (so the binary keeps human
/// rendering and its prompts are visible), and the shell still ends up in the
/// new directory.
// trace:BUG-0445 | ai:claude
#[test]
fn on_a_real_terminal_the_init_child_sees_a_tty_and_the_shell_still_cds() {
    if !util_linux_script_runs() {
        return;
    }
    for &shell in wrapper_shells() {
        let (transcript, code) =
            run_wrapper_in_pty(shell, &side_channel_stub(0), &init_body("proj"));
        assert_eq!(code, Some(0), "[{shell}] {transcript}");
        assert!(
            transcript.contains("probe:tty notpipe"),
            "[{shell}] the child must see the terminal on its stdout:\n{transcript}"
        );
        assert!(
            transcript.contains("Initializing proj"),
            "[{shell}] prose must render on the terminal:\n{transcript}"
        );
        assert!(
            transcript.contains("moved:yes"),
            "[{shell}] the cd must still apply:\n{transcript}"
        );
    }
}

// ── discipline: when the cd must NOT apply ───────────────────────────────────

/// A failing init leaves the shell where it was, propagates its status, and
/// still cleans up — even though the stub wrote a well-formed block.
// trace:BUG-0445 | ai:claude
#[test]
fn a_failing_init_never_moves_the_shell() {
    for &shell in wrapper_shells() {
        let (stdout, stderr, _) = run_wrapper_in(shell, &side_channel_stub(1), &init_body("proj"));
        assert!(
            stdout.contains("rc:1"),
            "[{shell}] status propagates:\n{stdout}"
        );
        assert!(
            stdout.contains("moved:no"),
            "[{shell}] a failed init must not cd:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            stderr.contains("error: forge refused"),
            "[{shell}] the real error reaches the operator on stderr:\n{stderr}"
        );
        assert!(stdout.contains("file:gone"), "[{shell}] {stdout}");
    }
}

/// Only a file holding exactly one marker pair and nothing else is eval'd.
/// Bare payloads, prose around the block, and a second pair are all rejected.
// trace:BUG-0445 | ai:claude
#[test]
fn a_malformed_side_channel_file_is_never_evaled() {
    let cd = "\"cd '$PWD/$2'\"";
    let cases = [
        ("bare payload, no markers", cd.to_string()),
        (
            "prose before the block",
            format!("prose '{EVAL_BEGIN}' {cd} '{EVAL_END}'"),
        ),
        (
            "prose after the block",
            format!("'{EVAL_BEGIN}' {cd} '{EVAL_END}' trailing"),
        ),
        (
            "two marker pairs",
            format!("'{EVAL_BEGIN}' {cd} '{EVAL_END}' '{EVAL_BEGIN}' 'export AIDA_T_TWICE=1' '{EVAL_END}'"),
        ),
        ("end before begin", format!("'{EVAL_END}' {cd} '{EVAL_BEGIN}'")),
        ("unterminated block", format!("'{EVAL_BEGIN}' {cd}")),
    ];
    for (label, content) in &cases {
        for &shell in wrapper_shells() {
            let body = format!(
                "{}echo \"twice:${{AIDA_T_TWICE-unset}}\"\n",
                init_body("proj")
            );
            let (stdout, stderr, _) = run_wrapper_in(shell, &file_content_stub(content), &body);
            assert!(
                stdout.contains("rc:0"),
                "[{shell}/{label}] a malformed file is not an error for the operator:\n{stdout}\n{stderr}"
            );
            assert!(
                stdout.contains("moved:no"),
                "[{shell}/{label}] must not eval:\n{stdout}\nstderr:\n{stderr}"
            );
            assert!(
                stdout.contains("twice:unset"),
                "[{shell}/{label}] no part of a rejected file may be eval'd:\n{stdout}"
            );
            assert!(
                stdout.contains("file:gone"),
                "[{shell}/{label}] cleanup happens on every path:\n{stdout}"
            );
        }
    }
}

/// Version skew: an older binary that still emits markers on STDOUT (or a bare
/// in-place init that prints prose) is display-only. Nothing on stdout is an
/// eval candidate in the init arm, and the shell stays put.
// trace:BUG-0445 | ai:claude
#[test]
fn stdout_markers_from_an_older_binary_are_display_only() {
    for &shell in wrapper_shells() {
        let (stdout, stderr, _) = run_wrapper_in(shell, &stdout_only_stub(), &init_body("proj"));
        assert!(stdout.contains("rc:0"), "[{shell}] {stdout}\n{stderr}");
        assert!(
            stdout.contains("moved:no"),
            "[{shell}] stdout is never eval'd by the init arm:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            stdout.contains("project ready:") && stdout.contains(EVAL_BEGIN),
            "[{shell}] an older binary's stdout is displayed as-is:\n{stdout}"
        );
        assert!(!stderr.contains("command not found"), "[{shell}] {stderr}");
    }
}

// ── bash and zsh must not diverge ────────────────────────────────────────────

/// Same stubs, every shell on the machine, byte-identical stdout/stderr/status
/// against the bash baseline — so a zsh-only difference in the file-reading
/// loop or the per-command env prefix fails here, not in a zsh user's terminal.
// trace:BUG-0445 | ai:claude
#[test]
fn init_side_channel_agrees_across_shells() {
    let cases: [(&str, String); 3] = [
        ("successful init", side_channel_stub(0)),
        ("failing init", side_channel_stub(1)),
        ("stdout-only older binary", stdout_only_stub()),
    ];
    let body = init_body("proj");
    for (label, stub) in &cases {
        let baseline = run_wrapper_in("bash", stub, &body);
        for &shell in wrapper_shells() {
            if shell == "bash" {
                continue;
            }
            let actual = run_wrapper_in(shell, stub, &body);
            assert_eq!(
                actual.0, baseline.0,
                "[{label}] {shell} stdout differs from bash\n{shell}:\n{}\nbash:\n{}",
                actual.0, baseline.0
            );
            assert_eq!(
                actual.1, baseline.1,
                "[{label}] {shell} stderr differs from bash\n{shell}:\n{}\nbash:\n{}",
                actual.1, baseline.1
            );
            assert_eq!(
                actual.2, baseline.2,
                "[{label}] {shell} status differs from bash"
            );
        }
    }
}

// ── interrupts ───────────────────────────────────────────────────────────────

/// A stub that interrupts its OWN calling shell with `signal` while init is in
/// the foreground (the way a Ctrl-C or a TERM reaches the wrapper's shell),
/// then exits as an interrupted child would.
// trace:BUG-1960 | ai:claude
fn interrupting_stub(signal: &str) -> String {
    let code = if signal == "INT" { 130 } else { 143 };
    format!(
        "#!/usr/bin/env bash\n\
         [ -n \"${env}\" ] && [ -e \"${env}\" ] && printf 'saw-file\\n' >&2\n\
         kill -s {signal} \"$PPID\"\n\
         exit {code}\n",
        env = INIT_CD_FILE_ENV,
    )
}

/// The outcome of one interrupt scenario: the shell's stdout, its exit code
/// (`None` when a signal ended it), and the side-channel files left in the
/// private TMPDIR afterwards.
// trace:BUG-1960 | ai:claude
struct InterruptRun {
    stdout: String,
    stderr: String,
    code: Option<i32>,
    leftover: Vec<String>,
}

/// Like `run_wrapper_in`, but `mktemp` writes into a TMPDIR this test owns,
/// and it is inspected AFTER the shell exits: a shell that dies mid-init
/// cannot report on its own leftovers.
// trace:BUG-1960 | ai:claude
fn run_interrupt_scenario(shell: &str, stub: &str, body: &str) -> InterruptRun {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let tmp = dir.path().join("tmp");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&tmp).unwrap();
    crate::test_exec::write_executable(&dir.path().join("aida"), stub);
    let script = format!(
        "PATH='{path}':\"$PATH\"\nexport PATH\n{helpers}\n{body}\n",
        path = dir.path().display(),
        helpers = crate::dev_cmd::SHELL_HELPERS,
    );
    let mut cmd = std::process::Command::new(shell);
    if shell == "zsh" {
        cmd.arg("-f");
    }
    // Its own process group, so a stub that signals its whole group (the
    // Ctrl-C shape) can never reach the test harness.
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
    let out = cmd
        .arg("-c")
        .arg(&script)
        .env_clear()
        .env("HOME", &home)
        .env("TMPDIR", &tmp)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(dir.path())
        .output_retrying_etxtbsy()
        .unwrap_or_else(|e| panic!("{shell} available: {e}"));
    let leftover = std::fs::read_dir(&tmp)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("aida-init-cd."))
        .collect();
    InterruptRun {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        code: out.status.code(),
        leftover,
    }
}

/// INT or TERM delivered to the shell while init runs, with no trap of the
/// caller's own: the signal must still take effect (the shell dies, exactly
/// as it would without the wrapper) and the private file must be gone. A
/// cleanup trap that swallowed the signal would let the script carry on.
// trace:BUG-1960 | ai:claude
#[test]
fn an_interrupt_during_init_kills_the_shell_and_leaves_no_side_channel_file() {
    for signal in ["INT", "TERM"] {
        for &shell in wrapper_shells() {
            let run = run_interrupt_scenario(
                shell,
                &interrupting_stub(signal),
                "aida init proj\necho after-init\n",
            );
            let ctx = format!(
                "[{shell}/{signal}] code={:?} leftover={:?}\n{}\nstderr:\n{}",
                run.code, run.leftover, run.stdout, run.stderr
            );
            assert!(
                run.stderr.contains("saw-file"),
                "the stub must have seen the file: {ctx}"
            );
            assert!(
                run.leftover.is_empty(),
                "an interrupted init must not leave its file behind: {ctx}"
            );
            assert!(
                !run.stdout.contains("after-init"),
                "the signal must not be swallowed: the shell has to die during init: {ctx}"
            );
            // Both shells re-raise the recorded signal at themselves once
            // the caller's (default) disposition is back, so the kernel ends
            // them: no exit code.
            assert!(
                run.code.is_none(),
                "the shell must be ended by the signal: {ctx}"
            );
        }
    }
}

/// With a caller trap installed, the wrapper re-delivers the signal after
/// restoring it, so in both shells the caller's handler still runs and
/// decides what happens next (here: carry on). Either way the file is gone.
// trace:BUG-1960 | ai:claude
#[test]
fn an_interrupt_during_init_reaches_the_callers_own_trap() {
    for signal in ["INT", "TERM"] {
        for &shell in wrapper_shells() {
            let body =
                format!("trap 'echo caller-trap' {signal}\naida init proj\necho after-init\n");
            let run = run_interrupt_scenario(shell, &interrupting_stub(signal), &body);
            let ctx = format!(
                "[{shell}/{signal}] code={:?} leftover={:?}\n{}\nstderr:\n{}",
                run.code, run.leftover, run.stdout, run.stderr
            );
            assert!(
                run.leftover.is_empty(),
                "an interrupted init must not leave its file behind: {ctx}"
            );
            assert!(
                run.stdout.contains("caller-trap\nafter-init"),
                "the caller's trap must run once the wrapper's is gone: {ctx}"
            );
        }
    }
}

/// A SUCCESSFUL init leaves the caller's traps exactly as they were: the
/// caller's INT trap is still installed, and no cleanup trap of the
/// wrapper's survives on INT or TERM.
// trace:BUG-1960 | ai:claude
#[test]
fn a_successful_init_restores_the_callers_traps() {
    for &shell in wrapper_shells() {
        let body = "trap 'echo caller-trap' INT\n\
                    aida init proj\n\
                    echo \"rc:$?\"\n\
                    if [ -n \"${ZSH_VERSION:-}\" ]; then trap; else trap -p INT TERM; fi\n\
                    if [ -n \"${ZSH_VERSION:-}\" ]; then functions TRAPINT TRAPTERM >/dev/null 2>&1 && echo trap-fn-leaked; fi\n\
                    if [ -n \"${ZSH_VERSION:-}\" ]; then functions _aida_init_zsh >/dev/null 2>&1 && echo helper-fn-leaked; fi\n";
        let run = run_interrupt_scenario(shell, &side_channel_stub(0), body);
        let ctx = format!(
            "[{shell}] code={:?} leftover={:?}\n{}\nstderr:\n{}",
            run.code, run.leftover, run.stdout, run.stderr
        );
        assert!(run.stdout.contains("rc:0"), "{ctx}");
        assert!(
            run.stdout.contains("trap -- 'echo caller-trap' SIGINT")
                || run.stdout.contains("trap -- 'echo caller-trap' INT"),
            "the caller's INT trap must still be installed: {ctx}"
        );
        assert!(
            !run.stdout.contains("_aida_cd_file") && !run.stdout.contains("TERM"),
            "no wrapper trap may outlive the call: {ctx}"
        );
        assert!(!run.stdout.contains("trap-fn-leaked"), "{ctx}");
        // trace:TASK-1344 | ai:claude
        assert!(
            !run.stdout.contains("helper-fn-leaked"),
            "the zsh helper must not stay defined in the caller's shell: {ctx}"
        );
        assert!(run.leftover.is_empty(), "{ctx}");
    }
}

/// The signal must be re-raised at the shell PROCESS running the wrapper, not
/// at the top-level script: `$$` names the top-level shell even inside a
/// subshell, so re-raising with it would kill the parent and let the
/// interrupted subshell carry on. Here the stub signals its own parent, the
/// subshell; the subshell must die and the parent must see it die.
// trace:BUG-1960 | ai:claude
#[test]
fn an_interrupt_during_init_in_a_subshell_ends_the_subshell_not_the_parent() {
    for signal in ["INT", "TERM"] {
        let code = if signal == "INT" { 130 } else { 143 };
        for &shell in wrapper_shells() {
            let body = "( aida init proj; echo \"sub-after:$?\" )\necho \"parent-alive:$?\"\n";
            let run = run_interrupt_scenario(shell, &interrupting_stub(signal), body);
            let ctx = format!(
                "[{shell}/{signal}] code={:?} leftover={:?}\n{}\nstderr:\n{}",
                run.code, run.leftover, run.stdout, run.stderr
            );
            assert!(
                run.stdout.contains(&format!("parent-alive:{code}")),
                "the parent must survive and see the subshell killed by {signal}: {ctx}"
            );
            assert!(
                !run.stdout.contains("sub-after"),
                "the interrupted subshell must not carry on: {ctx}"
            );
            assert!(run.leftover.is_empty(), "{ctx}");
        }
    }
}

/// A stub that delivers SIGINT to its whole PROCESS GROUP, the way a terminal
/// Ctrl-C does: the wrapper's shell and the child both receive it. It refuses
/// to signal unless the group is the one `run_interrupt_scenario` created for
/// the shell, so it can never reach a process outside the test.
// trace:BUG-1960 | ai:claude
fn group_interrupting_stub() -> String {
    format!(
        "#!/usr/bin/env bash\n\
         [ -n \"${env}\" ] && [ -e \"${env}\" ] && printf 'saw-file\\n' >&2\n\
         _pg=$(ps -o pgid= -p $$ | tr -d ' ')\n\
         [ \"$_pg\" = \"$PPID\" ] || {{ printf 'wrong-group\\n' >&2; exit 99; }}\n\
         kill -s INT -- -\"$_pg\"\n\
         exit 130\n",
        env = INIT_CD_FILE_ENV,
    )
}

/// SIGINT to the whole process group (shell and child together): with no
/// caller trap the shell dies of it; with one, the caller's trap runs and the
/// script continues. Neither leaves the private file behind.
// trace:BUG-1960 | ai:claude
#[test]
fn a_process_group_interrupt_during_init_behaves_like_ctrl_c() {
    for &shell in wrapper_shells() {
        for (body, trapped) in [
            ("aida init proj\necho after-init\n", false),
            (
                "trap 'echo caller-trap' INT\naida init proj\necho after-init\n",
                true,
            ),
        ] {
            let run = run_interrupt_scenario(shell, &group_interrupting_stub(), body);
            let ctx = format!(
                "[{shell} trapped={trapped}] code={:?} leftover={:?}\n{}\nstderr:\n{}",
                run.code, run.leftover, run.stdout, run.stderr
            );
            assert!(run.stderr.contains("saw-file"), "{ctx}");
            assert!(!run.stderr.contains("wrong-group"), "{ctx}");
            assert!(run.leftover.is_empty(), "{ctx}");
            if trapped {
                assert!(run.stdout.contains("caller-trap\nafter-init"), "{ctx}");
            } else {
                assert!(!run.stdout.contains("after-init"), "{ctx}");
                assert!(run.code.is_none(), "the shell must die of SIGINT: {ctx}");
            }
        }
    }
}

/// An interrupt that lands while `mktemp` is creating the private file (the
/// window before the child starts) must not strand the file either: the
/// cleanup handlers are installed before `mktemp` runs, the child is skipped,
/// and the signal still ends the shell. A stub `mktemp` on PATH creates the
/// file and then signals the top-level shell before returning its path.
// trace:TASK-1343 | ai:claude
#[test]
fn an_interrupt_while_the_side_channel_file_is_created_leaves_no_file() {
    for &shell in wrapper_shells() {
        let body = "cat > ./mktemp <<'AIDA_STUB'\n\
                    #!/bin/sh\n\
                    f=\"$TMPDIR/aida-init-cd.raced\"\n\
                    : > \"$f\"\n\
                    printf '%s\\n' \"$f\"\n\
                    kill -s INT \"$AIDA_TEST_TOP\"\n\
                    AIDA_STUB\n\
                    chmod +x ./mktemp\n\
                    AIDA_TEST_TOP=$$\n\
                    export AIDA_TEST_TOP\n\
                    aida init proj\n\
                    echo after-init\n";
        let run = run_interrupt_scenario(shell, "#!/bin/sh\necho child-ran >&2\n", body);
        let ctx = format!(
            "[{shell}] code={:?} leftover={:?}\n{}\nstderr:\n{}",
            run.code, run.leftover, run.stdout, run.stderr
        );
        assert!(
            run.leftover.is_empty(),
            "an interrupt during mktemp must not strand the file: {ctx}"
        );
        assert!(
            !run.stderr.contains("child-ran"),
            "an interrupt before the child starts must skip it: {ctx}"
        );
        assert!(
            !run.stdout.contains("after-init"),
            "the signal must not be swallowed: {ctx}"
        );
    }
}
