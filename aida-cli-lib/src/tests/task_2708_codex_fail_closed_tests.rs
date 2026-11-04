//! TASK-2708 (advisor ruling 2026-11-03, finding 2): trusted
//! `[contained.vendors] codex = "aida"` FAILS CLOSED on every Codex launch
//! surface that is not the receipt-owning headless boundary.
// trace:TASK-2708 | ai:claude
use super::*;
use std::path::{Path, PathBuf};

/// How one production Codex spawn path is kept closed under the AIDA posture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Guard {
    /// Finalizes through `codex_tool_env::prepare_launch`, which refuses by
    /// name when the trusted policy resolves Codex to `aida`.
    Finalizer,
    /// The receipt-owning boundary (`containment::launch::PreparedLaunch`)
    /// for the AIDA posture; every other posture still finalizes.
    ReceiptBoundaryAndFinalizer,
}

/// EVERY production Codex spawn path, by enclosing function. A new
/// `prepare_launch` / `PreparedLaunch::prepare` call site fails the count
/// check below until it is audited and recorded here.
const CODEX_SPAWN_PATHS: &[(&str, &str, Guard)] = &[
    // STORY-2254 detached `aida agent new codex --bg` worker.
    (
        "agent_codex_background.rs",
        "build_worker_exec",
        Guard::Finalizer,
    ),
    // `aida agent new codex` registry-launched bg child.
    ("agent_launch.rs", "agent_new_bg_dispatch", Guard::Finalizer),
    // Interactive `aida agent new codex` (foreground tracked child).
    ("agent_launch.rs", "run_agent_generation", Guard::Finalizer),
    (
        "compete_cmd.rs",
        "build_compete_arm_command",
        Guard::Finalizer,
    ),
    ("dispatch.rs", "launch_implementer_lane", Guard::Finalizer),
    // Phase-driver advisor consult.
    ("phase_driver.rs", "spawn_advisor_session", Guard::Finalizer),
    // Phase-driver CI-fix child.
    ("phase_driver_trait.rs", "attempt_ci_fix", Guard::Finalizer),
    (
        "session.rs",
        "spawn_vendor_headless_forking",
        Guard::ReceiptBoundaryAndFinalizer,
    ),
    ("session.rs", "exec_codex_session", Guard::Finalizer),
    ("session.rs", "finalize_codex_resume", Guard::Finalizer),
    (
        "session.rs",
        "spawn_reviewer_launch_plan_granted",
        Guard::Finalizer,
    ),
    (
        "session.rs",
        "finalize_coordinator_launch",
        Guard::Finalizer,
    ),
    (
        "session.rs",
        "run_vendor_headless_child",
        Guard::ReceiptBoundaryAndFinalizer,
    ),
    (
        "session.rs",
        "spawn_vendor_headless_resume_granted",
        Guard::Finalizer,
    ),
    // `aida do` guided session.
    (
        "zen_drive_root.rs",
        "guided_launch_command",
        Guard::Finalizer,
    ),
];

const FINALIZER_CALL: &str = "crate::codex_tool_env::prepare_launch(";
const RECEIPT_CALL: &str = "crate::containment::launch::PreparedLaunch::prepare(";

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" {
                sources(&path, out);
            }
        } else if name.ends_with(".rs")
            && name != "tests.rs"
            && !name.ends_with("_tests.rs")
            && !name.starts_with("test_")
        {
            out.push(path);
        }
    }
}

/// The source of `fn name(..)` up to the next item-level `fn` line.
fn fn_body<'a>(text: &'a str, name: &str) -> &'a str {
    let needle = format!("fn {name}(");
    let start = text
        .find(&needle)
        .unwrap_or_else(|| panic!("spawn path `{name}` not found"));
    let rest = &text[start + needle.len()..];
    let mut end = rest.len();
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let t = line.trim_start();
        if offset > 0
            && (t.starts_with("fn ")
                || t.starts_with("pub fn ")
                || t.starts_with("pub(crate) fn ")
                || t.starts_with("pub(super) fn "))
        {
            end = offset;
            break;
        }
        offset += line.len();
    }
    &rest[..end]
}

// trace:TASK-2708 | ai:claude
#[test]
fn task_2708_every_codex_spawn_path_is_inventoried_and_guarded() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    let mut found = std::collections::BTreeMap::<String, (usize, usize)>::new();
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        let counts = (
            text.matches(FINALIZER_CALL).count(),
            text.matches(RECEIPT_CALL).count(),
        );
        if counts != (0, 0) {
            let rel = path
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            found.insert(rel, counts);
        }
    }
    let mut expected = std::collections::BTreeMap::<String, (usize, usize)>::new();
    for (file, _, guard) in CODEX_SPAWN_PATHS {
        let e = expected.entry((*file).to_string()).or_default();
        e.0 += 1;
        if *guard == Guard::ReceiptBoundaryAndFinalizer {
            e.1 += 1;
        }
    }
    assert_eq!(
        found, expected,
        "a Codex spawn path was added or removed: audit it for the AIDA posture \
         (finalize through prepare_launch, or own a receipt) and record it in CODEX_SPAWN_PATHS"
    );
    for (file, name, guard) in CODEX_SPAWN_PATHS {
        let text = std::fs::read_to_string(src.join(file)).unwrap();
        let body = fn_body(&text, name);
        assert!(
            body.contains(FINALIZER_CALL),
            "{file}::{name} no longer finalizes through prepare_launch"
        );
        if *guard == Guard::ReceiptBoundaryAndFinalizer {
            let at = body
                .find(RECEIPT_CALL)
                .unwrap_or_else(|| panic!("{file}::{name} lost its receipt-owning boundary"));
            assert!(
                body[at..].contains("containment_applied(vendor, true, Some(run.receipt()))"),
                "{file}::{name}: the AIDA arm must require an accepted receipt"
            );
            // Only the AIDA posture skips the finalizer.
            assert!(
                body.contains("!= ContainmentPosture::Aida"),
                "{file}::{name}: only the AIDA posture may bypass prepare_launch"
            );
        }
    }
    // The refusal is the finalizer's first statement, before any rebuild.
    let finalizer = fn_body(include_str!("../codex_tool_env.rs"), "prepare_launch");
    assert!(finalizer
        .trim_start_matches(|c: char| c != '{')
        .trim_start_matches('{')
        .trim_start()
        .starts_with("refuse_unwired_aida_posture(child)?;"));
}

fn trusted_policy(root: &Path, codex: &str) {
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        format!("[contained.vendors]\ncodex = \"{codex}\"\n"),
    )
    .unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["config", "user.name", "task-2708"],
        vec!["config", "user.email", "task-2708@example.invalid"],
        vec!["add", ".aida/config.toml"],
        vec!["commit", "-q", "-m", "trusted"],
        vec!["update-ref", "refs/remotes/origin/main", "HEAD"],
        vec![
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    ] {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(&args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// Behavior: under trusted `codex = "aida"` the finalizer refuses with the
/// named error whatever the surface stamped (`none`, `native`, nothing, or
/// `aida`) and however the child names its project (env or cwd). Positive
/// control: the same commands pass the guard under `codex = "native"`.
// trace:TASK-2708 | ai:claude
#[test]
fn task_2708_trusted_aida_refuses_unwired_codex_whatever_the_stamp() {
    let aida = tempfile::tempdir().unwrap();
    trusted_policy(aida.path(), "aida");
    let native = tempfile::tempdir().unwrap();
    trusted_policy(native.path(), "native");
    for stamp in [None, Some("none"), Some("native"), Some("aida")] {
        for via_env in [true, false] {
            let build = |root: &Path| {
                let mut cmd = Command::new("codex");
                if via_env {
                    cmd.env("AIDA_PROJECT_ROOT", root);
                } else {
                    cmd.current_dir(root);
                }
                if let Some(stamp) = stamp {
                    cmd.env("AIDA_CONTAINMENT_POSTURE", stamp);
                }
                cmd
            };
            let error = prepare_launch(&mut build(aida.path()))
                .unwrap_err()
                .to_string();
            assert!(
                error.contains(UNWIRED_AIDA_POSTURE),
                "stamp {stamp:?} via_env {via_env}: {error}"
            );
            let control = refuse_unwired_aida_posture(&build(native.path()));
            assert_eq!(
                control.is_ok(),
                stamp != Some("aida"),
                "stamp {stamp:?} via_env {via_env}: {control:?}"
            );
        }
    }
}

/// Surface: the guided `aida do` Codex child refuses under `codex = "aida"`.
// trace:TASK-2708 | ai:claude
#[test]
fn task_2708_guided_codex_session_refuses_under_trusted_aida() {
    let root = tempfile::tempdir().unwrap();
    trusted_policy(root.path(), "aida");
    let launch = crate::session::GuidedSessionLaunch {
        program: "codex",
        prompt: "prompt".into(),
    };
    let error = crate::zen_drive_root::guided_launch_command(&launch, root.path()).unwrap_err();
    assert!(
        format!("{error:#}").contains(UNWIRED_AIDA_POSTURE),
        "{error:#}"
    );
}

/// Surface: the detached `--bg` Codex lane refuses the AIDA posture by name.
// trace:TASK-2708 | ai:claude
#[test]
fn task_2708_detached_codex_lane_refuses_under_trusted_aida() {
    let src = include_str!("../agent_codex_background.rs");
    let preflight = src
        .split("session::containment_posture_in(project_root, \"codex\", true)?;")
        .nth(1)
        .expect("detached lane resolves the trusted posture");
    let at_aida = preflight
        .find("posture != session::ContainmentPosture::Aida")
        .expect("detached lane refuses the AIDA posture");
    let named = preflight
        .find("crate::codex_tool_env::UNWIRED_AIDA_POSTURE")
        .expect("detached lane refusal is the named error");
    let native = preflight.find("ContainmentPosture::Native").unwrap();
    assert!(at_aida < named && named < native);
}

/// Fail closed (advisor ruling finding 6): a trusted policy that cannot be
/// loaded refuses the unwired launch by name, whatever the stamp; it never
/// degrades to "no policy".
// trace:TASK-2708 | ai:claude
#[test]
fn task_2708_trusted_policy_load_error_refuses_unwired_codex() {
    // An out-of-range value, and a body that is not TOML at all.
    for codex in ["sideways", "x\"\n[broken"] {
        let root = tempfile::tempdir().unwrap();
        trusted_policy(root.path(), codex);
        assert!(
            crate::agent_config::load_contained_vendor(root.path(), "codex").is_err(),
            "fixture must be unloadable: {codex:?}"
        );
        for stamp in [None, Some("none"), Some("native")] {
            let mut cmd = Command::new("codex");
            cmd.env("AIDA_PROJECT_ROOT", root.path());
            if let Some(stamp) = stamp {
                cmd.env("AIDA_CONTAINMENT_POSTURE", stamp);
            }
            let error = refuse_unwired_aida_posture(&cmd).unwrap_err().to_string();
            assert!(
                error.contains(UNWIRED_AIDA_POSTURE),
                "{codex:?} stamp {stamp:?}: {error}"
            );
        }
    }
}

/// Fail closed: when the child names no project and the main worktree cannot
/// be resolved, only a stamp proving a non-AIDA posture admits the launch.
// trace:TASK-2708 | ai:claude
#[test]
fn task_2708_unresolved_root_refuses_unless_stamp_proves_non_aida() {
    for stamp in [
        None,
        Some("aida"),
        Some("bogus"),
        Some("none"),
        Some("native"),
    ] {
        let mut cmd = Command::new("codex");
        if let Some(stamp) = stamp {
            cmd.env("AIDA_CONTAINMENT_POSTURE", stamp);
        }
        let result =
            refuse_unwired_aida_posture_with(&cmd, || anyhow::bail!("not inside a git repository"));
        assert_eq!(
            result.is_ok(),
            matches!(stamp, Some("none" | "native")),
            "stamp {stamp:?}: {result:?}"
        );
        if let Err(error) = result {
            assert!(error.to_string().contains(UNWIRED_AIDA_POSTURE), "{error}");
        }
    }
}
