//! Integration tests for the launcher's intent emission (STORY-244).
//!
//! These exercise the public `Intent` + `write_to_fd` surface end-to-end:
//! a temp file stands in for the bash wrapper's fd 3, the launcher's
//! serializer writes one intent line, and we read it back to confirm the
//! wire format the wrapper consumes. The full TUI event loop is not
//! exercised here (it needs a real terminal); see the launcher's unit
//! tests for the route_key / act_on_row coverage.
//!
//! trace:STORY-244 | ai:claude

#![cfg(unix)]

use std::io::{Read, Seek, SeekFrom};
use std::os::fd::AsRawFd;

fn temp_intent_file() -> std::fs::File {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(std::env::temp_dir().join(format!(
            "aida-launcher-intent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )))
        .expect("open temp intent file")
}

fn read_back(mut f: std::fs::File) -> String {
    f.seek(SeekFrom::Start(0)).unwrap();
    let mut s = String::new();
    f.read_to_string(&mut s).unwrap();
    s
}

#[test]
fn launcher_serializes_quit_intent_with_newline() {
    // The launcher's quit path is the simplest end-to-end check:
    // write a Quit intent to a non-stdio fd and confirm the wire
    // format the bash wrapper's `case "$intent" in quit) break` arm
    // expects.
    let f = temp_intent_file();
    let fd = f.as_raw_fd() as u32;
    aida_tui::__test_only::write_intent(&aida_tui::__test_only::Intent::Quit, fd)
        .expect("write succeeds");
    assert_eq!(read_back(f), "quit\n");
}

#[test]
fn launcher_serializes_launch_intent_for_queue_work() {
    let f = temp_intent_file();
    let fd = f.as_raw_fd() as u32;
    aida_tui::__test_only::write_intent(
        &aida_tui::__test_only::Intent::Launch("aida queue work STORY-244".to_string()),
        fd,
    )
    .expect("write succeeds");
    let out = read_back(f);
    assert!(
        out.starts_with("launch:"),
        "wire prefix must be `launch:` so the bash wrapper's case arm matches; got {out:?}"
    );
    assert!(out.contains("aida queue work STORY-244"));
    assert!(out.ends_with('\n'));
}

#[test]
fn launcher_serializes_resume_intent_for_session() {
    let f = temp_intent_file();
    let fd = f.as_raw_fd() as u32;
    aida_tui::__test_only::write_intent(
        &aida_tui::__test_only::Intent::Resume("019e2d4f-7777-7abc".to_string()),
        fd,
    )
    .expect("write succeeds");
    let out = read_back(f);
    assert_eq!(out, "resume:019e2d4f-7777-7abc\n");
}

#[test]
fn launcher_rejects_shell_metacharacters() {
    let f = temp_intent_file();
    let fd = f.as_raw_fd() as u32;
    let err = aida_tui::__test_only::write_intent(
        &aida_tui::__test_only::Intent::Launch("rm -rf / && touch /tmp/x".to_string()),
        fd,
    )
    .expect_err("must reject metacharacters");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("disallowed"),
        "error should explain the rejection; got {msg:?}"
    );
}
