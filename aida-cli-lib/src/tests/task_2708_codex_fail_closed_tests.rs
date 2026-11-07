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
    // dispatch.rs take/impl Codex lanes (BUG-2390) spawn no Codex directly:
    // `build_codex_lane_command` runs `aida agent new codex --bg`, whose
    // worker is `agent_codex_background.rs::build_worker_exec` above. Pinned
    // by `DISPATCH_CODEX_LANE` below.
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

/// The dispatch take/impl Codex lane builder (BUG-2390): a detached `aida
/// agent new codex --bg` launch, guarded by `build_worker_exec`'s finalizer.
const DISPATCH_CODEX_LANE: &str = "build_codex_lane_command";

const FINALIZER_CALL: &str = "crate::launch::profile::finalize_codex(";
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
    // The coordinator adds a Claude-only receipt boundary. Codex still
    // finalizes first and must not gain access to that Claude arm.
    // trace:TASK-3219 | ai:codex
    let session = std::fs::read_to_string(src.join("session.rs")).unwrap();
    let coordinator = fn_body(&session, "coordinator_status_with_handle_guard");
    let finalize = coordinator
        .find("finalize_coordinator_launch(vendor, cmd, project_root)?")
        .unwrap();
    let claude = coordinator
        .find("if vendor == HeadlessVendor::Claude")
        .unwrap();
    let receipt = coordinator.find(RECEIPT_CALL).unwrap();
    assert!(finalize < claude && claude < receipt);
    assert!(coordinator[receipt..].contains("bind_contained_launch_worker(&grant, run.receipt())?"));
    expected.get_mut("session.rs").unwrap().1 += 1;
    // S2 adds a separately named Claude interactive preparation seam. Codex
    // still finalizes before it could reach that arm; the selection is Claude-only.
    let agent = std::fs::read_to_string(src.join("agent_launch.rs")).unwrap();
    let generation = fn_body(&agent, "run_agent_generation");
    let selection = generation
        .find("let wrapper = config.agent_type == \"claude\"")
        .unwrap();
    let finalize = generation.find(FINALIZER_CALL).unwrap();
    let interactive = generation
        .find("PreparedLaunch::prepare_interactive(")
        .unwrap();
    assert!(selection < finalize && finalize < interactive);
    assert!(
        generation[interactive..].contains("bind_contained_launch_worker(grant, run.receipt())?")
    );
    assert!(generation[interactive..].contains("Some(run.receipt())"));

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
    // Dispatch's Codex lane is the detached `aida agent new codex --bg`
    // launch (finalized in build_worker_exec), never a direct Codex spawn.
    let dispatch = include_str!("../dispatch.rs");
    let lane = fn_body(dispatch, DISPATCH_CODEX_LANE);
    for arg in ["\"agent\"", "\"new\"", "\"codex\"", "\"--bg\""] {
        assert!(
            lane.contains(arg),
            "dispatch.rs::{DISPATCH_CODEX_LANE} no longer routes through `aida agent new codex --bg`"
        );
    }
    assert!(
        fn_body(dispatch, "build_implementer_launch_command")
            .contains("DispatchVendor::Codex => {\n            unreachable!("),
        "dispatch.rs::build_implementer_launch_command grew a direct Codex arm: \
         finalize it through prepare_launch and record it in CODEX_SPAWN_PATHS"
    );
    // The refusal is the finalizer's first statement, before any rebuild.
    let finalizer = fn_body(include_str!("../launch/profile.rs"), "prepare_launch");
    assert!(finalizer
        .trim_start_matches(|c: char| c != '{')
        .trim_start_matches('{')
        .trim_start()
        .starts_with("refuse_unwired_aida_posture(child)?;"));
}

/// How a `crate::launch::spawn_agent` builder (STORY-2326) reaches a Codex
/// guard. `spawn_agent` only constructs the child `Command` (it never spawns),
/// so each builder is closed by the inventoried spawn path that finalizes it.
#[derive(Debug, Clone, Copy)]
enum SpawnAgentGuard {
    /// The builder is itself a `CODEX_SPAWN_PATHS` entry.
    Itself,
    /// Finalized by the `CODEX_SPAWN_PATHS` entry `(file, fn)`.
    FinalizedBy(&'static str, &'static str),
    /// Builds Claude only (an `interactive_claude_context`).
    ClaudeOnly,
    /// Dispatch's Codex arm is unreachable; the detached control route is
    /// pinned separately by DISPATCH_CODEX_LANE above.
    DispatchControl,
}

/// EVERY production `crate::launch::spawn_agent(` call site, by enclosing
/// function. A new call site fails the count check below until it is audited
/// and recorded here.
const SPAWN_AGENT_BUILDERS: &[(&str, &str, SpawnAgentGuard)] = &[
    // PR-C: the guided vendor child now shares the launcher and its profile.
    (
        "zen_drive_root.rs",
        "guided_launch_command",
        SpawnAgentGuard::Itself,
    ),
    // PR-B: each new constructor retains its finalizer after seat setup.
    // trace:STORY-2326 | ai:codex
    (
        "compete_cmd.rs",
        "build_compete_arm_command",
        SpawnAgentGuard::Itself,
    ),
    (
        "dispatch.rs",
        "build_implementer_launch_command",
        SpawnAgentGuard::DispatchControl,
    ),
    (
        "phase_driver.rs",
        "build_phase_driver_advisor_command",
        SpawnAgentGuard::FinalizedBy("phase_driver.rs", "spawn_advisor_session"),
    ),
    (
        "phase_driver_trait.rs",
        "build_phase_driver_ci_fix_command",
        SpawnAgentGuard::FinalizedBy("phase_driver_trait.rs", "attempt_ci_fix"),
    ),
    (
        "session.rs",
        "build_codex_session_command",
        SpawnAgentGuard::FinalizedBy("session.rs", "exec_codex_session"),
    ),
    ("session.rs", "exec_codex_session", SpawnAgentGuard::Itself),
    (
        "agent_launch.rs",
        "build_agent_new_bg_command",
        SpawnAgentGuard::FinalizedBy("agent_launch.rs", "agent_new_bg_dispatch"),
    ),
    (
        "agent_launch.rs",
        "run_agent_generation",
        SpawnAgentGuard::Itself,
    ),
    (
        "session.rs",
        "build_claude_exec_command",
        SpawnAgentGuard::ClaudeOnly,
    ),
    (
        "session.rs",
        "build_run_claude_session_command",
        SpawnAgentGuard::ClaudeOnly,
    ),
    (
        "session.rs",
        "spawn_vendor_headless_forking",
        SpawnAgentGuard::Itself,
    ),
    // `aida questions clarify` advisor: run through
    // `coordinator_status_with_handle_guard`, which finalizes.
    (
        "session.rs",
        "build_advisor_interactive_command",
        SpawnAgentGuard::FinalizedBy("session.rs", "finalize_coordinator_launch"),
    ),
    (
        "session.rs",
        "spawn_reviewer_launch_plan_granted",
        SpawnAgentGuard::Itself,
    ),
    // Coordinators (burndown, autopilot intake) finalize through
    // `finalize_coordinator_launch`; the drain's exec path through
    // `run_vendor_headless_child`'s receipt boundary.
    (
        "session.rs",
        "build_coordinator_headless_command_inner",
        SpawnAgentGuard::FinalizedBy("session.rs", "run_vendor_headless_child"),
    ),
];

const SPAWN_AGENT_CALL: &str = "crate::launch::spawn_agent(";

// trace:TASK-2708 | ai:claude
#[test]
fn task_2708_every_spawn_agent_builder_is_inventoried_and_guarded() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    let mut found = std::collections::BTreeMap::<String, usize>::new();
    for path in files {
        let n = std::fs::read_to_string(&path)
            .unwrap()
            .matches(SPAWN_AGENT_CALL)
            .count();
        if n > 0 {
            let rel = path.strip_prefix(&src).unwrap().to_string_lossy();
            found.insert(rel.into_owned(), n);
        }
    }
    let mut expected = std::collections::BTreeMap::<String, usize>::new();
    for (file, _, _) in SPAWN_AGENT_BUILDERS {
        *expected.entry((*file).to_string()).or_default() += 1;
    }
    assert_eq!(
        found, expected,
        "a spawn_agent call site was added or removed: audit it for the Codex AIDA \
         posture and record it in SPAWN_AGENT_BUILDERS"
    );
    let inventoried = |file: &str, name: &str| {
        CODEX_SPAWN_PATHS
            .iter()
            .any(|(f, n, _)| *f == file && *n == name)
    };
    for (file, name, guard) in SPAWN_AGENT_BUILDERS {
        let text = std::fs::read_to_string(src.join(file)).unwrap();
        let body = fn_body(&text, name);
        assert!(
            body.contains(SPAWN_AGENT_CALL),
            "{file}::{name} no longer builds through spawn_agent"
        );
        match guard {
            SpawnAgentGuard::Itself => assert!(
                inventoried(file, name),
                "{file}::{name} is not a CODEX_SPAWN_PATHS entry"
            ),
            SpawnAgentGuard::FinalizedBy(f, n) => assert!(
                inventoried(f, n),
                "{file}::{name}: its finalizer {f}::{n} is not a CODEX_SPAWN_PATHS entry"
            ),
            SpawnAgentGuard::DispatchControl => assert!(
                body.contains("DispatchVendor::Codex =>") && body.contains("unreachable!("),
                "dispatch must route Codex through its detached AIDA control child"
            ),
            SpawnAgentGuard::ClaudeOnly => assert!(
                body.contains("interactive_claude_context("),
                "{file}::{name} no longer builds a Claude-only context"
            ),
        }
    }
    // spawn_agent constructs; it never spawns or execs a child itself.
    let spawn = fn_body(include_str!("../launch/spawn.rs"), "spawn_agent");
    for forbidden in [".spawn()", ".exec()", ".status()", ".output()"] {
        assert!(
            !spawn.contains(forbidden),
            "launch::spawn_agent must only construct the child Command ({forbidden})"
        );
    }
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
            let error = crate::launch::profile::finalize_codex(&mut build(aida.path()))
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

/// M1: a named root whose trusted sha cannot be resolved (not a git
/// repository) is NOT "no policy": it refuses by name unless the child's own
/// stamp proves a non-AIDA posture, whether the root is named by env or cwd.
// trace:TASK-2708 | ai:claude
#[test]
fn task_2708_non_repo_named_root_refuses_unless_stamp_proves_non_aida() {
    let root = tempfile::tempdir().unwrap();
    assert!(crate::trusted_config::trusted_default_branch_sha(root.path()).is_none());
    for stamp in [
        None,
        Some("aida"),
        Some("bogus"),
        Some("none"),
        Some("native"),
    ] {
        for via_env in [true, false] {
            let mut cmd = Command::new("codex");
            if via_env {
                cmd.env("AIDA_PROJECT_ROOT", root.path());
            } else {
                cmd.current_dir(root.path());
            }
            if let Some(stamp) = stamp {
                cmd.env("AIDA_CONTAINMENT_POSTURE", stamp);
            }
            let result = refuse_unwired_aida_posture(&cmd);
            assert_eq!(
                result.is_ok(),
                matches!(stamp, Some("none" | "native")),
                "stamp {stamp:?} via_env {via_env}: {result:?}"
            );
            if let Err(error) = result {
                assert!(error.to_string().contains(UNWIRED_AIDA_POSTURE), "{error}");
            }
        }
    }
}

/// M1 positive control: a repository whose trusted sha resolves but carries
/// no `.aida/config.toml` there is a real "no policy" answer, so an
/// unstamped child passes the guard.
// trace:TASK-2708 | ai:claude
#[test]
fn task_2708_no_config_at_trusted_sha_admits_unstamped_codex() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("README"), "no aida config here").unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["config", "user.name", "task-2708"],
        vec!["config", "user.email", "task-2708@example.invalid"],
        vec!["add", "README"],
        vec!["commit", "-q", "-m", "no config"],
        vec!["update-ref", "refs/remotes/origin/main", "HEAD"],
        vec![
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    ] {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(root.path())
            .args(&args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
    }
    assert!(crate::trusted_config::trusted_default_branch_sha(root.path()).is_some());
    let mut cmd = Command::new("codex");
    cmd.env("AIDA_PROJECT_ROOT", root.path());
    refuse_unwired_aida_posture(&cmd).unwrap();
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
