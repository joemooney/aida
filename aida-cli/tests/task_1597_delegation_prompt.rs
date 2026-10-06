use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::Path;

mod support;

fn setup_repo(home: &Path, repo: &Path) {
    std::fs::create_dir_all(home).unwrap();
    std::fs::create_dir_all(repo).unwrap();
    std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(repo)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "you@example.com"])
        .current_dir(repo)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Your Name"])
        .current_dir(repo)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-q", "--allow-empty", "-m", "init"])
        .current_dir(repo)
        .status()
        .unwrap();

    let bin = env!("CARGO_BIN_EXE_aida");
    std::process::Command::new(bin)
        .args(["init", "--no-skills", "--no-hooks", "--no-agent-config"])
        .env("HOME", home)
        .env("AIDA_HEADLESS", "1")
        .current_dir(repo)
        .output()
        .unwrap();

    support::ensure_seat(home, repo, "advisor", &[]);
}

#[test]
fn test_task_1597_delegation_prompt_and_recovery_hint() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    setup_repo(&home, &repo);

    let bin = env!("CARGO_BIN_EXE_aida");

    // 1. Headless no-prompt check
    let headless = std::process::Command::new(bin)
        .arg("role")
        .arg("enter")
        .arg("advisor")
        .env("AIDA_HEADLESS", "1")
        .env("HOME", &home)
        .current_dir(&repo)
        .output()
        .unwrap();
    let stderr = String::from_utf8(headless.stderr).unwrap();
    assert!(
        !headless.status.success(),
        "should exit with non-zero status in headless"
    );
    assert!(
        !stderr.contains("Include it in the TTY-issued delegation set?"),
        "should not prompt headless"
    );

    // 2. TTY interaction using portable-pty
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new(bin);
    cmd.arg("role");
    cmd.arg("enter");
    cmd.arg("advisor");
    cmd.env("HOME", &home);
    cmd.env(
        "USER",
        std::env::var("USER").unwrap_or_else(|_| "joe".to_string()),
    );
    cmd.cwd(&repo);

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();

    std::thread::spawn(move || {
        // Sleep briefly to let the prompt render
        std::thread::sleep(std::time::Duration::from_millis(500));
        // Decline the prompt
        writer
            .write_all(
                b"n
",
            )
            .unwrap();
        // Drop writer so EOF is sent
        drop(writer);
    });

    let mut output = String::new();
    reader.read_to_string(&mut output).unwrap();

    assert!(child.wait().unwrap().success());
    assert!(
        output.contains("TTY-issued delegation"),
        "should prompt at TTY"
    );

    // Verify recovery hint on sub-launch failure
    let mut grant_id = String::new();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("export AIDA_SESSION_GRANT='") {
            if let Some(id) = rest.strip_suffix("'") {
                grant_id = id.to_string();
                break;
            }
        }
    }
    assert!(
        !grant_id.is_empty(),
        "could not find AIDA_SESSION_GRANT in pty output"
    );

    std::process::Command::new(bin)
        .args([
            "add", "--type", "task", "--title", "test", "--status", "approved",
        ])
        .env("HOME", &home)
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("AIDA_SESSION_GRANT", &grant_id)
        .current_dir(&repo)
        .status()
        .unwrap();

    let sub = std::process::Command::new(bin)
        .arg("questions")
        .arg("clarify")
        .arg("TASK-1") // The first spec added above
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("AIDA_SESSION_GRANT", &grant_id)
        .env("HOME", &home)
        .current_dir(&repo)
        .output()
        .unwrap();

    let sub_stderr = String::from_utf8(sub.stderr).unwrap();
    let sub_stdout = String::from_utf8(sub.stdout).unwrap();
    println!("sub_stderr: {}\nsub_stdout: {}", sub_stderr, sub_stdout);
    assert!(sub_stdout.contains("does not delegate `advisor`"));

    // 3. Managed agent with TTY gets immediate refusal, no prompt
    let pty_system2 = native_pty_system();
    let pair2 = pty_system2
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd2 = CommandBuilder::new(bin);
    cmd2.arg("role");
    cmd2.arg("enter");
    cmd2.arg("advisor");
    cmd2.env("HOME", &home);
    cmd2.env("AIDA_AGENT_NAME", "codex");
    cmd2.env(
        "USER",
        std::env::var("USER").unwrap_or_else(|_| "joe".to_string()),
    );
    cmd2.cwd(&repo);

    let mut child2 = pair2.slave.spawn_command(cmd2).unwrap();
    drop(pair2.slave);

    let mut reader2 = pair2.master.try_clone_reader().unwrap();
    // we don't need to write to it because it should immediately refuse
    let mut output2 = String::new();
    reader2.read_to_string(&mut output2).unwrap();

    let status2 = child2.wait().unwrap();
    assert!(!status2.success(), "should exit with non-zero status");
    assert!(
        !output2.contains("Include it in the TTY-issued delegation set?"),
        "should not prompt managed agents"
    );
    assert!(
        output2.contains("AIDA-managed agent sessions cannot issue direct TTY grants"),
        "should show the managed agent refusal message"
    );

    assert!(sub_stdout.contains("aida role enter advisor --delegate-seat advisor"));
}
