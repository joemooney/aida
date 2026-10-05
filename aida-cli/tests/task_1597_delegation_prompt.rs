#![cfg(unix)]
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::Path;

mod support;

fn setup_repo(home: &Path, repo: &Path) {
    std::fs::create_dir_all(home).unwrap();
    std::fs::create_dir_all(repo).unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo)
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-q", "--allow-empty", "-m", "init"])
        .current_dir(repo)
        .status()
        .unwrap();
    // Add the user to the roster so they have the advisor seat.
    std::fs::create_dir_all(repo.join(".aida-store/team")).unwrap();
    let user = std::env::var("USER").unwrap_or_else(|_| "joe".to_string());
    let roster = format!(
        r#"
members:
  - id: {}
    name: Test User
    seats:
      - advisor
"#,
        user
    );
    std::fs::write(repo.join(".aida-store/team/roster.yaml"), roster).unwrap();
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

    // Read the prompt in a non-blocking way or using a thread
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let mut output = String::new();
        let mut buf = [0u8; 1024];
        let mut found = false;
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            output.push_str(std::str::from_utf8(&buf[..n]).unwrap_or(""));
            if !found && output.contains("TTY-issued delegation") {
                found = true;
                tx.send(output.clone()).unwrap();
            }
        }
        std::fs::write("debug_pty.log", output).unwrap();
    });

    // Wait for the prompt
    let res = rx.recv_timeout(std::time::Duration::from_secs(5));
    assert!(
        res.is_ok(),
        "should prompt at TTY, output log written to debug_pty.log"
    );

    // Decline the prompt
    writer
        .write_all(
            b"n
",
        )
        .unwrap();

    // Drop writer so EOF is sent if needed
    drop(writer);

    assert!(child.wait().unwrap().success());

    // Verify recovery hint on sub-launch failure
    let sub = std::process::Command::new(bin)
        .arg("decide")
        .arg("--clarify")
        .env("AIDA_SESSION_ROLE", "advisor")
        // No grant
        .env("HOME", &home)
        .current_dir(&repo)
        .output()
        .unwrap();

    let sub_stderr = String::from_utf8(sub.stderr).unwrap();
    assert!(sub_stderr.contains("does not delegate `advisor`"));
    assert!(sub_stderr.contains("aida role enter advisor --delegate-seat advisor"));
}
