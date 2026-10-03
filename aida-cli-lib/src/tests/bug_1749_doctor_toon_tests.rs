use crate::{
    doctor_cmd::{self, DoctorRenderMode},
    OutputFormat,
};
use std::process::Command;

// trace:BUG-1749 | ai:codex
#[test]
fn doctor_mode_resolution_truth_table() {
    use DoctorRenderMode::*;
    assert_eq!(doctor_cmd::doctor_render_mode_from(false, None, true), Toon);
    assert_eq!(
        doctor_cmd::doctor_render_mode_from(false, None, false),
        Human
    );
    assert_eq!(doctor_cmd::doctor_render_mode_from(true, None, true), Json);
    assert_eq!(
        doctor_cmd::doctor_render_mode_from(false, Some(OutputFormat::Json), true),
        Json
    );
    assert_eq!(
        doctor_cmd::doctor_render_mode_from(false, Some(OutputFormat::Toon), false),
        Toon
    );
    assert_eq!(
        doctor_cmd::doctor_render_mode_from(false, Some(OutputFormat::Human), true),
        Human
    );
}

// trace:BUG-1749 | ai:codex
#[test]
fn doctor_toon_fixture_round_trips_scalars_and_tables() {
    let report = doctor_cmd::bug_1745_test_report();
    let rendered = doctor_cmd::render_doctor_report_toon(&report);
    assert_eq!(
        crate::toon::parse_scalar(rendered.lines().next().unwrap()),
        Some(("total".into(), "1".into()))
    );
    let table_start = rendered.find("findings[").unwrap();
    let table_end = rendered[table_start..]
        .find("\nerror:")
        .map(|n| table_start + n)
        .unwrap_or(rendered.len());
    let parsed = crate::toon::parse_table(&rendered[table_start..table_end]).unwrap();
    assert_eq!(parsed.name, "findings");
    assert_eq!(parsed.rows[0][0], "performance");
    assert!(rendered.lines().any(|line| crate::toon::parse_scalar(line)
        == Some(("error".into(), "synthetic failure reason".into()))));
    assert!(rendered.contains("performance_audits[1]"));
}

// Copy the ownership-checked resolver harness from BUG-1745. The resolver is
// intentional: TASK-1262's architecture guard exact-allowlists its literal call;
// do not name the underlying OS lookup here.
// trace:BUG-1749 | ai:codex
fn built_aida_binary() -> std::path::PathBuf {
    let runner = crate::resolve_aida_exe();
    let deps = runner.parent().expect("test runner has a parent directory");
    assert_eq!(deps.file_name().and_then(|name| name.to_str()), Some("deps"), "expected this build's runner under target/<profile>/deps; an ambient AIDA_BIN redirects the resolver");
    let binary = deps
        .parent()
        .expect("deps has a target/<profile> parent")
        .join("aida");
    let checkout = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let is_in_checkout = binary.starts_with(checkout);
    let is_in_target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(|dir| binary.starts_with(std::path::PathBuf::from(dir)))
        .unwrap_or(false);
    assert!(
        is_in_checkout || is_in_target_dir,
        "refusing to exercise a binary outside this checkout"
    );
    assert!(binary.is_file(), "build aida-cli before running this test");
    binary
}

// trace:BUG-1749 | ai:codex
fn run(args: &[&str]) -> std::process::Output {
    Command::new(built_aida_binary())
        .args(args)
        .output()
        .unwrap()
}

// trace:BUG-1749 | ai:codex
#[test]
fn performance_failure_stdout_is_one_parseable_toon_document() {
    let out = run(&["doctor", "check", "performance", "--fail-on-findings"]);
    assert!(!out.status.success(), "fixture must exercise a finding");
    let text = String::from_utf8(out.stdout).unwrap();
    let lines = text.lines().collect::<Vec<_>>();
    let total = lines
        .iter()
        .find_map(|line| crate::toon::parse_scalar(line))
        .filter(|(key, _)| key == "total")
        .expect("total scalar is present")
        .1
        .parse::<usize>()
        .unwrap();
    assert!(total > 0);
    let start = text.find("findings[").expect("findings table is emitted");
    let end = text[start..]
        .find("\nerror:")
        .map(|n| start + n)
        .unwrap_or(text.len());
    let finding = crate::toon::parse_table(&text[start..end]).expect("one findings table parses");
    assert_eq!(finding.name, "findings");
    let errors = text
        .lines()
        .filter_map(crate::toon::parse_scalar)
        .filter(|(key, _)| key == "error")
        .collect::<Vec<_>>();
    assert_eq!(
        errors.len(),
        1,
        "stdout must contain exactly one error field"
    );
    assert!(errors[0].1.contains("failing because --fail-on-findings"));
    assert!(!text.contains("─── AIDA doctor ───"));
}

// trace:BUG-1749 | ai:codex
#[test]
fn json_spellings_match_and_human_pin_stays_on_stderr() {
    let flag = run(&[
        "doctor",
        "check",
        "performance",
        "--fail-on-findings",
        "--json",
    ]);
    let pin = run(&[
        "--format",
        "json",
        "doctor",
        "check",
        "performance",
        "--fail-on-findings",
    ]);
    assert_eq!(flag.stdout, pin.stdout);
    assert!(!flag.status.success() && !pin.status.success());
    let human = run(&[
        "--format",
        "human",
        "doctor",
        "check",
        "performance",
        "--fail-on-findings",
    ]);
    assert!(!human.status.success());
    assert!(String::from_utf8_lossy(&human.stdout).contains("─── AIDA doctor ───"));
    assert!(String::from_utf8_lossy(&human.stderr).contains("Error:"));
}
