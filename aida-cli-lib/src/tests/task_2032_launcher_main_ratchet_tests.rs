//! TASK-2032 round 3: `launcher_main()` resolves the host main from the
//! process cwd, so it is right only at a launch's entry point, before any
//! chdir into a seat. After `queue work` or `session start` chdirs into a
//! seat clone it returns `None` and every boundary built from it loses the
//! clone (its `.git` denied, its target dir ungranted). Boundary builders
//! therefore take the host main as a parameter, and this ratchet lists the
//! only functions allowed to call `launcher_main()`: entry points that run at
//! a host cwd. A new builder that reaches for it fails here.
//! trace:TASK-2032 | ai:claude

use std::path::Path;

/// `(file, function)` pairs allowed to call `launcher_main()` in production
/// code. Each runs at a host cwd, never after a chdir into a seat.
const ENTRY_POINTS: &[(&str, &str)] = &[
    // Launch entry points that chdir into a seat: checked below to resolve
    // the host main BEFORE their chdir.
    ("session_start.rs", "session_start_with_receipt"),
    ("queue_cmd/work.rs", "handle_queue_work"),
    // `queue work --no-launch` hints, rendered before any chdir.
    ("queue_cmd/work.rs", "headless_launch_hint"),
    ("queue_cmd/work.rs", "deferred_interactive_launch_hint"),
    ("queue_cmd/work.rs", "build_queue_work_claude_command"),
    // Host-cwd entry wrappers in the session module.
    ("session.rs", "spawn_vendor_headless_with_seat"),
    ("session.rs", "spawn_codex_headless_fork"),
    ("session.rs", "exec_resume_command"),
    // `aida agent new` / `aida session`, launched from the host project.
    ("agent_launch.rs", "handle_session_command"),
    ("agent_launch.rs", "agent_resume_launch_config"),
    ("agent_launch.rs", "rescope_contained_settings"),
    ("agent_launch.rs", "wrapped_agent_program_and_args"),
    ("agent_launch.rs", "build_agent_new_bg_command"),
    ("agent_launch.rs", "run_agent_generation_with_io"),
    ("lib_part2.rs", "apply_agent_default_flags"),
    // Coordinators and the orchestrator run at the host project root.
    ("autopilot_intake.rs", "handle_intake_command"),
    ("autopilot_intake.rs", "build_autopilot_intake_command"),
    ("burndown_run.rs", "handle_burndown_run"),
    ("burndown_run.rs", "run_burndown_verbose"),
    ("burndown_run.rs", "build_burndown_quiet_command"),
    ("burndown_run.rs", "build_burndown_verbose_command"),
    ("compete_cmd.rs", "build_compete_arm_command"),
    // `aida compete --dry-run` previews the arm argv at the host root.
    ("compete_cmd.rs", "handle_compete_command"),
    ("phase_driver.rs", "spawn_advisor_session"),
    ("phase_driver_trait.rs", "attempt_ci_fix"),
    ("phase_driver_trait.rs", "resume_implementer"),
    ("advisor_watch.rs", "fork_and_run_to"),
    ("questions.rs", "questions_clarify"),
    ("questions.rs", "build_questions_clarify_command"),
    ("lib_part5.rs", "review_spec_launch_reviewer"),
    ("leases.rs", "lease_tree_dirty_entries"),
];

/// The entry points that chdir into a seat, and the chdir each must follow
/// its `launcher_main()` call.
// Keep the source needles split: these are strings to inspect, not cwd
// mutations for the test isolation source guard to reject.
// trace:TASK-2032 | ai:codex
const CHDIR_ENTRY_POINTS: &[(&str, &str, &str)] = &[
    (
        "session_start.rs",
        "session_start_with_receipt",
        concat!("std::env::", "set_current_dir", "(&worktree_path)"),
    ),
    (
        "queue_cmd/work.rs",
        "handle_queue_work",
        concat!("std::env::", "set_current_dir", "(&lease.worktree_path)"),
    ),
];

const CALL: &str = "crate::seat_clone::launcher_main()";

/// Every production `launcher_main()` call: `(file, enclosing fn, line)`.
/// A file's trailing `#[cfg(test)] mod tests` block, and the `tests/`
/// directory, are test code and skipped.
fn production_calls(src: &Path) -> Vec<(String, String, usize)> {
    let fn_re =
        regex::Regex::new(r"^\s*(?:pub(?:\([a-z]+\))?\s+)?(?:async\s+)?fn\s+(\w+)").unwrap();
    let mut calls = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(src)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .collect();
    if src.join("queue_cmd").is_dir() {
        files.extend(
            std::fs::read_dir(src.join("queue_cmd"))
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|p| p.extension().is_some_and(|x| x == "rs")),
        );
    }
    files.sort();
    for path in files {
        let name = path
            .strip_prefix(src)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let mut current = String::new();
        for (i, line) in lines.iter().enumerate() {
            if line.starts_with("mod tests") && i > 0 && lines[i - 1].trim() == "#[cfg(test)]" {
                break;
            }
            if let Some(c) = fn_re.captures(line) {
                current = c[1].to_string();
            }
            // trace:TASK-3429 | ai:codex — external crates use the public library path.
            if (line.contains(CALL) || line.contains("aida_cli_lib::seat_clone::launcher_main()"))
                && !line.trim_start().starts_with("//")
            {
                calls.push((name.clone(), current.clone(), i + 1));
            }
        }
    }
    calls
}

fn src_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[test]
fn task_2032_launcher_main_is_called_only_from_entry_points() {
    // trace:TASK-3429 | ai:codex — retain coverage of the extracted transport.
    let mut calls = production_calls(&src_dir());
    calls.extend(production_calls(&src_dir().join("../../aida-mcp/src")));
    assert!(
        calls.len() >= 10,
        "the scan found too few calls ({}); did `launcher_main` move?",
        calls.len()
    );
    let offenders: Vec<String> = calls
        .iter()
        .filter(|(file, func, _)| !ENTRY_POINTS.iter().any(|(f, n)| f == file && n == func))
        .map(|(file, func, line)| format!("{file}:{line} in `{func}`"))
        .collect();
    assert!(
        offenders.is_empty(),
        "`launcher_main()` derives the host main from the process cwd, which \
         names the seat after a chdir into one. Take the host main as a \
         parameter, resolved by the launch's entry point before any chdir \
         (TASK-2032); list a function here only if it is such an entry \
         point:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn task_2032_chdir_entry_points_resolve_the_host_main_before_the_chdir() {
    let src = src_dir();
    for (file, func, chdir) in CHDIR_ENTRY_POINTS {
        let text = std::fs::read_to_string(src.join(file)).unwrap();
        let start = text
            .find(&format!("fn {func}("))
            .unwrap_or_else(|| panic!("{file}: no fn {func}"));
        let body = &text[start..];
        let chdir_at = body
            .find(chdir)
            .unwrap_or_else(|| panic!("{file}::{func}: no `{chdir}`"));
        let resolved_at = body
            .find(CALL)
            .unwrap_or_else(|| panic!("{file}::{func}: no host main resolution"));
        assert!(
            resolved_at < chdir_at,
            "{file}::{func} must resolve the host main before `{chdir}`"
        );
        let next_fn = body[chdir_at..]
            .find("\nfn ")
            .into_iter()
            .chain(body[chdir_at..].find("\npub(crate) fn "))
            .chain(body[chdir_at..].find("\npub fn "))
            .min()
            .map_or(body.len(), |at| chdir_at + at);
        assert!(
            !body[chdir_at..next_fn].contains(CALL),
            "{file}::{func} calls `launcher_main()` after `{chdir}`: inside the \
             seat it has no host main"
        );
    }
}
