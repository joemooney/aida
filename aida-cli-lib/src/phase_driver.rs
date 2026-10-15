//! phase driver: DrainConfigToml and read_drain_config, RealPhaseDriver with its config, agent gates and inherent impls, ci-fix prompt and AutonomyMode (STORY-1488 slice 1; Q9 split)
// trace:STORY-1488 | ai:claude

use crate::*;

/// Run `git -C <worktree> <args>` and capture trimmed stdout, or `None` on any
/// spawn / non-zero / decode error. trace:BUG-420 | ai:claude
pub(crate) fn git_capture(worktree: &std::path::Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The `[drain]` config values, all optional. trace:TASK-136 BUG-420 | ai:claude
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct DrainConfigToml {
    pub(crate) gh_verify_retries: Option<usize>,
    pub(crate) no_progress_minutes: Option<u64>,
    pub(crate) phase_ceiling_minutes: Option<u64>,
    pub(crate) spinning_after: Option<u64>,
    pub(crate) runaway_rate: Option<f64>,
    pub(crate) low_entropy_bits: Option<f64>,
    pub(crate) progress_activity: Option<u64>,
    /// TASK-975: `[drain] ci_auto_fix = N` — how many in-drain CI-fix cycles
    /// phase 2 may attempt on a red CI run before shelving. Also arms the
    /// phase-4 merge-conflict rebase when > 0. Absent/0 = off.
    pub(crate) ci_auto_fix: Option<usize>,
    /// STORY-975: `[drain] retry_transient = N` — whole-phase retries for
    /// transient typed shelve causes. Default 1, max 3.
    // trace:STORY-975 | ai:codex
    pub(crate) retry_transient: Option<usize>,
    /// STORY-1033: `[drain] retry_escalate_model = true|false`.
    pub(crate) retry_escalate_model: Option<bool>,
    /// STORY-1041: `[drain] pipeline_depth = 1..3`.
    // trace:STORY-1041 trace:ADR-27 | ai:codex
    pub(crate) pipeline_depth: Option<usize>,
    /// BUG-1275: tracked CI wait bounds. Durations accept seconds, `s`, or `m`.
    pub(crate) ci_idle: Option<u64>,
    pub(crate) ci_absolute: Option<u64>,
    /// STORY-1414: `[drain] require_head = true` — refuse a drain launch on a
    /// stale dev binary instead of warning.
    // trace:STORY-1414 | ai:claude
    pub(crate) require_head: Option<bool>,
}

/// Hand-rolled `[drain]`-section scanner for `.aida/config.toml`, mirroring the
/// other section readers (e.g. `read_behavior_permission_mode`) so the crate
/// stays serde-free for one small optional section. Unknown keys are ignored.
/// trace:TASK-136 BUG-420 | ai:claude
pub(crate) fn read_drain_config(project_dir: &std::path::Path) -> DrainConfigToml {
    let mut out = DrainConfigToml::default();
    let config_path = project_dir.join(".aida").join("config.toml");
    let Ok(content) = std::fs::read_to_string(&config_path) else {
        return out;
    };
    let mut in_drain = false;
    for raw in content.lines() {
        let line = strip_toml_inline_comment(raw).trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            in_drain = rest.trim_start_matches('[').starts_with("drain");
            continue;
        }
        if !in_drain {
            continue;
        }
        if let Some((key, val)) = line.split_once('=') {
            let key = key.trim();
            let val = val.trim().trim_matches('"').trim();
            match key {
                "gh_verify_retries" => out.gh_verify_retries = val.parse().ok(),
                "no_progress_minutes" => out.no_progress_minutes = val.parse().ok(),
                "phase_ceiling_minutes" => out.phase_ceiling_minutes = val.parse().ok(),
                "spinning_after" => out.spinning_after = parse_duration_seconds(val),
                "runaway_rate" => out.runaway_rate = val.parse().ok(),
                "low_entropy_bits" => out.low_entropy_bits = val.parse().ok(),
                "progress_activity" => out.progress_activity = parse_duration_seconds(val),
                // trace:TASK-975 | ai:claude
                "ci_auto_fix" => out.ci_auto_fix = val.parse().ok(),
                "retry_transient" => out.retry_transient = val.parse().ok(),
                "retry_escalate_model" => out.retry_escalate_model = parse_boolish(val),
                "pipeline_depth" => out.pipeline_depth = val.parse().ok(),
                "ci_idle" => out.ci_idle = parse_duration_seconds(val),
                "ci_absolute" => out.ci_absolute = parse_duration_seconds(val),
                "require_head" => out.require_head = parse_boolish(val),
                _ => {}
            }
        }
    }
    out
}

// trace:STORY-1033 | ai:codex
pub(crate) fn parse_boolish(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

// trace:STORY-998 | ai:codex
pub(crate) fn parse_duration_seconds(raw: &str) -> Option<u64> {
    let trimmed = raw.trim().trim_matches('"');
    if let Some(secs) = trimmed.strip_suffix('s') {
        return secs.trim().parse().ok();
    }
    if let Some(mins) = trimmed.strip_suffix('m') {
        return mins
            .trim()
            .parse::<u64>()
            .ok()
            .map(|m| m.saturating_mul(60));
    }
    trimmed.parse().ok()
}

pub(crate) struct RealPhaseDriver {
    pub(crate) project_root: std::path::PathBuf,
    /// BUG-1037: the auto-complete lifecycle forge, resolved once from the
    /// target project root and reused by phase consumers. This intentionally
    /// uses the open-change resolver so a stale pure-git config cannot make
    /// reviewer preflights disagree with auto-open on a GitHub-origin repo.
    // trace:BUG-1037 | ai:codex
    pub(crate) lifecycle_forge: crate::forge::ForgeKind,
    /// TASK-1421: injectable forge constructor. `None` in every production
    /// path (the driver falls back to `forge_for_kind`); a test sets it to
    /// observe the forge calls the publication boundary makes.
    // trace:TASK-1421 | ai:claude
    pub(crate) forge_factory: Option<crate::forge::ForgeFactory>,
    pub(crate) spec: String,
    /// Queue owner captured when the drain selected this pipeline member. Phase
    /// children keep this identity while their role changes per phase.
    // trace:BUG-1038 | ai:codex
    pub(crate) queue_user_id: String,
    pub(crate) permission_mode: Option<String>,
    /// BUG-2133: whether this drain is contained, resolved once at
    /// construction ([`session::drain_contained`]). Containment is monotonic:
    /// every agent child the drain spawns directly (CI-fix, advisor, harvest)
    /// inherits it, and a child that cannot be contained is refused.
    // trace:BUG-2133 | ai:claude
    pub(crate) contained: bool,
    pub(crate) json: bool,
    pub(crate) branch: Option<String>,
    pub(crate) implementer_lease: Option<String>,
    pub(crate) pr_number: Option<u32>,
    /// BUG-1244: PR proven by phase 1 (or explicitly seeded resume) for this
    /// run. CI/reviewer probes must remain on this PR.
    // trace:BUG-1244 | ai:codex
    pub(crate) phase_done_pr: Option<u32>,
    /// Exact commit produced by phase 1 and bound to `phase_done_pr`.
    // trace:BUG-1818 | ai:codex
    pub(crate) phase_done_head: Option<String>,
    pub(crate) ci_run_id: Option<String>,
    /// PR head for which phase 2 observed terminal CI. Phase 3 may only review
    /// this exact head and records the conclusion in its verdict artifact.
    // trace:BUG-1460 | ai:codex
    pub(crate) ci_terminal_sha: Option<String>,
    pub(crate) ci_terminal_green: Option<bool>,
    /// Cached aida binary path, resolved once at construction. Re-resolving
    /// per-call broke in BUG-217 when the implementer's phase-1 `cargo build`
    /// replaced the running dev binary: `/proc/self/exe` then includes a
    /// " (deleted)" suffix, and `Command::new("<path> (deleted)").spawn()`
    /// fails with ENOENT. trace:BUG-217 | ai:claude
    pub(crate) aida_exe: std::path::PathBuf,
    /// STORY-263: headless mode. `Some` → the reviewer phase is launched
    /// headless (`claude -p`); the implementer phase stays interactive in
    /// this cut (STORY-276 wires it). `None` → fully interactive.
    pub(crate) no_human: Option<auto_complete::NoHumanMode>,
    /// ADR-10: the run's typed autonomy mode, resolved ONCE at dispatch (via
    /// [`AutonomyMode::for_auto_complete_run`]) and carried here alongside
    /// `no_human` so the engine's in-process zen branches consult this typed
    /// field instead of re-reading the bare `AIDA_ZEN` env var. `AIDA_ZEN`
    /// survives strictly as the cross-process TRANSPORT to spawned phase
    /// children / skill templates; this field is the in-process SOURCE OF
    /// TRUTH. Symmetric with `no_human`, closing the `--zen`/`--no-human`
    /// asymmetry ADR-7 named.
    // trace:ADR-10 | ai:claude
    pub(crate) autonomy_mode: AutonomyMode,
    /// TASK-329: poll cadence + SIGTERM→SIGKILL grace for the graceful-exit
    /// signal. Resolved once from `AIDA_EXIT_POLL_MS` / `AIDA_EXIT_GRACE_MS`.
    pub(crate) exit_cfg: exit_signal::ExitSignalConfig,
    /// BUG-233: the corroboration token for this orchestrator run. Passed to
    /// every phase child as `AIDA_AUTO_COMPLETE_TOKEN` so the child can verify
    /// — against the live marker file — that its `AIDA_AUTO_COMPLETE=1` comes
    /// from a real orchestrator rather than guessing.
    pub(crate) run_token: String,
    /// STORY-306: the Claude `--session-id` the phase-1 implementer was
    /// launched with. When phase 1 punts and the advisor tier resolves the
    /// fork, `resume_implementer` `--resume`s exactly this session so the
    /// implementer re-enters its own conversation with the working model it
    /// had built. `None` until `run_implementer` mints it.
    pub(crate) implementer_session: Option<String>,
    /// STORY-306: the worktree the phase-1 implementer ran in. Required by
    /// `resume_implementer`: `claude --resume` finds the persisted session
    /// only when its cwd matches the cwd the session was created with (Claude
    /// derives the project slug under `~/.claude/projects/<slug>/` from cwd),
    /// so the resume must `current_dir(<this worktree>)`. `None` until
    /// `run_implementer` locates the lease.
    pub(crate) implementer_worktree: Option<std::path::PathBuf>,
    /// BUG-311: thread the user's `aida queue work --auto-complete --steal`
    /// flag through to the phase-1 implementer subprocess. Without this the
    /// outer `--steal` is dropped on the floor — phase 1's `handle_queue_work`
    /// sees the dormant lease, bails with the canned "pass --steal" message,
    /// and the orchestrator reports a generic phase-1 failure even though
    /// the user *did* pass --steal. trace:BUG-311 | ai:claude
    pub(crate) steal: bool,
    /// TASK-559: thread the user's `aida queue work --auto-complete
    /// --force-claim` flag through to the phase-1 implementer subprocess.
    pub(crate) force_claim: bool,
    /// STORY-281: opt out of the reviewer pre-flight stale-base refusal.
    /// When false (default), phase 3 refuses to launch the reviewer if the
    /// PR's base is behind origin AND a file the PR touches has moved on
    /// the base since the PR forked. When true, the refusal becomes a
    /// warning so review-against-stale proceeds. Also propagated to the
    /// reviewer subprocess so its own pre-flight respects the opt-out.
    /// trace:STORY-281 | ai:claude
    pub(crate) allow_stale_base: bool,
    /// STORY-429: opt out of phase-3 auto-rebase recovery.
    pub(crate) no_auto_rebase: bool,
    /// STORY-429: embedded telemetry for phase-3 stale-base auto-rebase.
    pub(crate) auto_rebase_events: Vec<auto_complete_telemetry::AutoRebaseEvent>,
    /// STORY-442: opt-in lifecycle short-circuit tags for non-integrity
    /// phases. Phase 2 owns CI waiting; phases 3/6 are sequenced in
    /// `auto_complete.rs`.
    pub(crate) lifecycle_skip: auto_complete::LifecycleSkip,
    /// BUG-742: the auto-complete stop mode this drive was launched with.
    /// Phase children receive it via `AIDA_AUTO_COMPLETE_VARIANT` so their
    /// pickup prompt says "open the PR and stop" instead of the direct-ship
    /// cadence. The merge bouncer does not trust this value; it only informs
    /// child-session guidance.
    // trace:BUG-742 | ai:codex
    pub(crate) variant: auto_complete::AutoCompleteVariant,
    /// BUG-868: this drive entered through `queue work --from-pr`, so phase 3
    /// must tell the reviewer that a Done spec is expected and the concrete
    /// contract is reviewing the already-open PR, not queue cleanup.
    // trace:BUG-868 | ai:codex
    pub(crate) from_pr: bool,
    /// TASK-136 / BUG-420: GH-verify retry budget + watchdog thresholds,
    /// resolved once from `[drain]` config + env/flag overrides.
    pub(crate) drain_tuning: DrainTuning,
    /// BUG-826: bounded retry count for an instant headless-vendor death that
    /// leaves a zero-byte log. This is phase-1 launch resilience, not a retry
    /// of real implementer work.
    // trace:BUG-826 | ai:codex
    pub(crate) empty_launch_retries_used: usize,
    /// BUG-908: a transient phase-1 retry re-enters the predecessor worktree
    /// after releasing its dead lease, instead of creating a fresh worktree and
    /// colliding with the dirty WIP the first attempt intentionally preserved.
    // trace:BUG-908 | ai:codex
    pub(crate) retry_implementer_worktree: Option<std::path::PathBuf>,
    pub(crate) retry_implementer_branch: Option<String>,
    /// BUG-1629: the phase-1 workspace pinned for launch — the value the
    /// orchestrator checked, never a second resolution.
    // trace:BUG-1629 | ai:claude
    pub(crate) phase1_workspace: Option<Phase1Workspace>,
    /// BUG-1629: the single clean replacement launch spent when a phase-1
    /// child exits without its session lease and handoff receipt.
    // trace:BUG-1629 | ai:claude
    pub(crate) lost_child_state_retries_used: usize,
    /// BUG-1769: the spec's store status read ONCE at phase-1 entry, before any
    /// child launches. [`RealPhaseDriver::recover_lost_child_state`] compares it
    /// against a fresh read to tell "this run finished the work" (the BUG-1140
    /// substrate verification) from the stale-Done refusal BUG-1524 must keep
    /// surfacing.
    ///
    /// Set by [`RealPhaseDriver::capture_phase1_entry_spec_status`], called at
    /// the top of `RealPhaseDriver::run_implementer`. The OUTER `Option` is the
    /// captured/not-captured flag, not a status: `run_implementer` is re-entered
    /// by the BUG-1629 replacement launch and the BUG-826 relaunches, and a
    /// second capture would overwrite the entry reading with a mid-run one and
    /// destroy the comparison. The INNER `Option` is `spec_status`'s own
    /// "no store, or the spec is not in it".
    // trace:BUG-1769 | ai:claude
    pub(crate) phase1_entry_spec_status: Option<Option<RequirementStatus>>,
    /// BUG-1213 / TASK-1265: `(PR, dispatched branch, blocking verdict's
    /// reviewed_sha, the dispatched branch's head AT ARM TIME, authoritative
    /// review delta, round)` captured immediately before a rework implementer
    /// runs. `None` for ordinary first-pass work (no blocking verdict exists,
    /// so the no-op guard must never fire).
    /// TASK-1449 (rework, common-path regression): `reviewed_sha` is
    /// `Option` because ~86% of the verdict corpus carries no sha. That is
    /// UNKNOWN, not "no refusal", but it must not become an automatic
    /// refusal either — `rework_no_op_failure` falls back to the arm-time
    /// head as the comparison baseline and only refuses when NEITHER is
    /// available.
    // trace:BUG-1213 trace:TASK-1265 trace:TASK-1449 | ai:claude
    pub(crate) rework_guard: Option<(u32, String, Option<String>, Option<String>, String, usize)>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Phase3StaleOverlapAction {
    Proceed,
    Refuse(auto_complete::PhaseFailure),
}

/// TASK-136: the outcome of one phase-1 PR-verify attempt. The verify can
/// settle definitively (a PR found, no PR, or a hard failure) or hit a
/// *transient* GH-API outage that leaves it unable to confirm a PR — only the
/// last case is retried (with the [`auto_complete::gh_verify_backoff_schedule`]
/// backoff). trace:TASK-136 | ai:claude
/// BUG-444: given that BOTH phase-1 PR lookups returned empty-but-successful,
/// decide whether that's a *definitive* no-PR (the branch was never pushed) or a
/// *retryable* eventual-consistency window (the branch is on origin, so a PR
/// likely exists but isn't indexed yet). Only an `Absent` branch is a definitive
/// NoPr; `Present` and `LsRemoteFailed` both retry. Pure so the keystone
/// decision is unit-tested without gh/git. trace:BUG-444 | ai:claude
pub(crate) fn empty_phase1_lookup_is_definitive_nopr(origin: &BranchOriginProbe) -> bool {
    matches!(origin, BranchOriginProbe::Absent)
}

pub(crate) enum Phase1PrResolve {
    /// An open PR was found (directly or recovered via the spec-id search).
    Found(OpenPrInfo),
    /// BUG-709: no OPEN PR, but the branch's PR is already MERGED — the
    /// implementer ran the full ship itself. The work landed; the drive
    /// completes cleanly rather than retrying the open-PR verify.
    // trace:BUG-709 | ai:claude
    AlreadyMerged(OpenPrInfo),
    /// Definitively no open PR — falls through to the punt / NoPr path.
    NoPr,
    /// A definitive hard failure (gh missing / gh errored / branch not on
    /// origin) — return it as the phase failure, no retry.
    Fail(auto_complete::PhaseFailure),
    /// Transient: GH was unreachable but the branch may carry a PR — retry,
    /// then declare Inconclusive with this reason if the budget is exhausted.
    Retry(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentGateOnFail {
    Shelve,
    Warn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentGateConfig {
    pub(crate) name: String,
    pub(crate) role: String,
    pub(crate) applies_to: String,
    pub(crate) on_fail: AgentGateOnFail,
}

pub(crate) fn normalize_gate_token(s: &str) -> String {
    s.trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

pub(crate) fn sanitize_gate_file_token(s: &str) -> String {
    let token: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let token = token.trim_matches('-');
    if token.is_empty() {
        "agent-gate".to_string()
    } else {
        token.to_string()
    }
}

pub(crate) fn parse_agent_gates_from_config(value: Option<&toml::Value>) -> Vec<AgentGateConfig> {
    let Some(gates) = value
        .and_then(|v| v.get("pipeline"))
        .and_then(|v| v.get("gate"))
        .and_then(|v| v.as_table())
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, gate) in gates {
        let kind = gate
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if !kind.eq_ignore_ascii_case("agent") {
            continue;
        }
        let Some(role) = gate.get("role").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(applies_to) = gate.get("applies_to").and_then(|v| v.as_str()) else {
            continue;
        };
        let on_fail = match gate
            .get("on_fail")
            .and_then(|v| v.as_str())
            .unwrap_or("shelve")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "warn" => AgentGateOnFail::Warn,
            _ => AgentGateOnFail::Shelve,
        };
        out.push(AgentGateConfig {
            name: name.to_string(),
            role: role.trim().to_string(),
            applies_to: applies_to.trim().to_string(),
            on_fail,
        });
    }
    out
}

pub(crate) fn selector_tokens(selector: &str) -> impl Iterator<Item = &str> {
    selector
        .split(|c: char| c == ',' || c == '|' || c == '/' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

pub(crate) fn agent_gate_matches_req(gate: &AgentGateConfig, req: &aida_core::Requirement) -> bool {
    selector_tokens(&gate.applies_to).any(|token| {
        if let Some(want) = token.strip_prefix("type:") {
            normalize_gate_token(want) == normalize_gate_token(&req.req_type.to_string())
                || normalize_gate_token(want) == normalize_gate_token(req.req_type.default_prefix())
        } else if let Some(want) = token.strip_prefix("tag:") {
            let want = want.trim();
            req.tags.iter().any(|tag| tag.eq_ignore_ascii_case(want))
        } else {
            false
        }
    })
}

pub(crate) fn matching_agent_gates_for_spec(
    project_root: &std::path::Path,
    spec: &str,
) -> Vec<AgentGateConfig> {
    let gates = parse_agent_gates_from_config(read_project_config_value(project_root).as_ref());
    if gates.is_empty() {
        return Vec::new();
    }
    let Some(store) = load_store_for_lookup(project_root) else {
        return Vec::new();
    };
    let Some(req) = store
        .requirements
        .iter()
        .find(|r| r.spec_id.as_deref() == Some(spec))
    else {
        return Vec::new();
    };
    gates
        .into_iter()
        .filter(|gate| agent_gate_matches_req(gate, req))
        .collect()
}

/// STORY-492: a `--resume-drain` re-entry context for [`run_auto_complete`] —
/// the reconciled phase to re-enter at plus the branch + PR to seed into the
/// driver (the skipped earlier phases would have discovered these). `None`
/// passed to `run_auto_complete` means a normal phase-1 drain.
/// trace:STORY-492 | ai:claude
pub(crate) struct ResumeEntry {
    pub(crate) start_phase: auto_complete::Phase,
    pub(crate) branch: Option<String>,
    pub(crate) pr: Option<u32>,
    /// Phase-1 head persisted in drain state. A resumed phase 2 compares the
    /// forge's current head to this value instead of blessing a moved change.
    // trace:BUG-1818 | ai:codex
    pub(crate) head_sha: Option<String>,
    pub(crate) from_pr: bool,
    /// BUG-1819: terminal CI evidence verified during `--from-pr`
    /// reconciliation. A direct reviewer re-entry must inherit this rather
    /// than pretending only an in-process phase 2 can establish it.
    pub(crate) ci_terminal_sha: Option<String>,
    pub(crate) ci_terminal_green: Option<bool>,
}

/// What the ownership probe concluded about a same-branch open change before
/// TASK-1529 retracts it. `Foreign` and `Unverified` both refuse the close, but
/// they are different operator situations: a change we could not identify may
/// still be ours and still carries failing guards, so it is reported as loudly
/// as a failed close, while somebody else's change is left alone quietly.
// trace:TASK-1529 | ai:claude
pub(crate) enum RetractionOwnership {
    /// Open, and its author is this forge identity.
    Ours,
    /// Open, and confirmed somebody else's.
    Foreign,
    /// The probe itself failed, or the forge withheld the author — the
    /// change may still be ours and may still be open.
    Unverified,
    /// Resolved as closed or merged: nothing to retract, nothing to hold.
    /// The AlreadyMerged path owns the merged case.
    // trace:TASK-1552 | ai:claude
    NoLongerOpen,
}

/// What `retract_publication_with_notice` left behind on the forge.
///
/// TASK-1552 finding 1: the refused-preflight caller must know not merely
/// whether the close happened, but whether a guards-failing change is STILL
/// OPEN — that is the case that needs a merge-hold, or the refusal is
/// enforced by stderr prose only.
// trace:TASK-1552 | ai:claude
pub(crate) enum RetractionOutcome {
    /// The open change was closed; nothing is published.
    Closed,
    /// A guards-failing change remains open (or could not be confirmed
    /// closed): foreign or unverified ownership, or the close call failed.
    LeftOpen(crate::forge::ChangeRef),
    /// Nothing to act on: no change found, or it is no longer open.
    Nothing,
}

/// Validate the phase-1 change binding before phase 2 uses its source branch.
/// Kept pure so exact-head and attribution failures stay regression-testable.
// trace:BUG-1818 | ai:codex
pub(crate) fn verified_phase2_branch(
    pr: u32,
    expected_head: &str,
    metadata: &crate::forge::ChangeMetadata,
    credit: PrCreditMatch,
    spec: &str,
) -> Result<String, auto_complete::PhaseFailure> {
    if metadata.state != crate::forge::ChangeState::Open {
        return Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::CiUnavailable,
            format!("phase-1 PR/MR {pr} is {:?}, not open", metadata.state),
        ));
    }
    if !expected_head.eq_ignore_ascii_case(metadata.head_sha.trim()) {
        return Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::CiUnavailable,
            format!(
                "phase-1 PR/MR {pr} head mismatch: produced `{expected_head}`, forge reports `{}`",
                metadata.head_sha
            ),
        ));
    }
    match credit {
        PrCreditMatch::Dispatched => {}
        PrCreditMatch::Other(other) => {
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::ShippedMismatch,
                format!(
                    "phase-1 PR/MR {pr} at `{}` credits {other}, not {spec}",
                    metadata.head_sha
                ),
            ));
        }
        PrCreditMatch::Unknown => {
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::ShippedMismatch,
                format!(
                    "could not verify that phase-1 PR/MR {pr} at `{}` covers {spec}; refusing phase 2",
                    metadata.head_sha
                ),
            ));
        }
    }
    if metadata.head_ref.trim().is_empty() {
        return Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::CiUnavailable,
            format!("phase-1 PR/MR {pr} did not report a source branch"),
        ));
    }
    Ok(metadata.head_ref.clone())
}

impl RealPhaseDriver {
    /// Set the PR number AND, if the spec is supervised, stamp the merge-hold
    /// marker the MOMENT the PR becomes known (fresh creation or recovery/detect)
    /// — not at the later hold-decision. Closes the BUG-1167 residual race where a
    /// concurrent/stale merger merged in the window between "PR goes green" and
    /// the drain's hold firing (STORY-1163 #1861 merged ~49s before its hold).
    /// record_merge_supervision_hold stays as the merge-time backstop; this is the
    /// early stamp every pr_number-set path funnels through.
    // trace:BUG-1173 | ai:claude
    pub(crate) fn set_pr_number(&mut self, pr: u32) {
        self.pr_number = Some(pr);
        if let Some(reason) = auto_complete::PhaseDriver::merge_supervision_hold(self) {
            let pr = u64::from(pr);
            let _ = crate::merge_hold::write_typed_hold(
                &self.project_root,
                &crate::merge_hold::typed_hold(
                    pr,
                    crate::merge_hold::HoldReasonKind::Supervision,
                    &reason,
                    None,
                )
                .with_spec(&self.spec), // trace:BUG-1562 | ai:claude
            );
            // BUG-1173 deferred the LABEL to merge time because an early red
            // merge-hold-gate read as a CI failure. Since BUG-1180 / ADR-39 the
            // drain's CI watch and `aida pr ship` classify that red as THE HOLD
            // when the marker exists, so the label is safe at PR-detection —
            // and BUG-1236 showed the merge-time sync silently never landed on
            // three supervised PRs, leaving Layer 2 off. Sync here, surface
            // failure, and let `aida merge-hold list --fix` repair it.
            // trace:BUG-1173 trace:BUG-1236 | ai:claude
            if let Err(err) = crate::merge_hold::sync_label(&self.project_root, pr, true) {
                eprintln!(
                    "  {} merge-hold label not applied on PR-{pr}: {err} — Layer 2 is off for this PR until `aida merge-hold list --fix` re-syncs it",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
        }
    }

    // why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        project_root: std::path::PathBuf,
        spec: String,
        queue_user_id: String,
        permission_mode: Option<String>,
        json: bool,
        no_human: Option<auto_complete::NoHumanMode>,
        // ADR-10: the run's typed autonomy mode, resolved ONCE at dispatch and
        // carried alongside `no_human`. trace:ADR-10 | ai:claude
        autonomy_mode: AutonomyMode,
        run_token: String,
        steal: bool,
        force_claim: bool,
        allow_stale_base: bool,
        no_auto_rebase: bool,
        lifecycle_skip: auto_complete::LifecycleSkip,
        variant: auto_complete::AutoCompleteVariant,
    ) -> Self {
        let drain_tuning = DrainTuning::resolve(&project_root);
        let lifecycle_forge = crate::forge::resolve_open_change_forge_kind(&project_root);
        let contained = session::drain_contained(&project_root);
        Self {
            project_root,
            lifecycle_forge,
            forge_factory: None,
            spec,
            queue_user_id,
            permission_mode,
            contained,
            json,
            branch: None,
            implementer_lease: None,
            pr_number: None,
            phase_done_pr: None,
            phase_done_head: None,
            ci_run_id: None,
            ci_terminal_sha: None,
            ci_terminal_green: None,
            aida_exe: resolve_aida_exe(),
            no_human,
            autonomy_mode,
            exit_cfg: exit_signal::ExitSignalConfig::from_env(),
            run_token,
            implementer_session: None,
            implementer_worktree: None,
            steal,
            force_claim,
            allow_stale_base,
            no_auto_rebase,
            auto_rebase_events: Vec::new(),
            lifecycle_skip,
            variant,
            from_pr: false,
            drain_tuning,
            empty_launch_retries_used: 0,
            retry_implementer_worktree: None,
            retry_implementer_branch: None,
            phase1_workspace: None,
            lost_child_state_retries_used: 0,
            phase1_entry_spec_status: None,
            rework_guard: None,
        }
    }

    pub(crate) fn sessions_dir(&self) -> std::path::PathBuf {
        self.project_root.join(".aida").join("sessions")
    }

    /// ADR-10: whether this run is a supervised `--zen` drive, read from the
    /// carried typed `autonomy_mode` field — the in-process source of truth —
    /// NOT from a bare `AIDA_ZEN` env re-read.
    // trace:ADR-10 | ai:claude
    pub(crate) fn is_zen_run(&self) -> bool {
        self.autonomy_mode.is_zen()
    }

    pub(crate) fn aida_exe(&self) -> std::path::PathBuf {
        self.aida_exe.clone()
    }

    /// TASK-1421: the injection seam. An injected
    /// [`crate::forge::ForgeFactory`] reaches the forges built through
    /// `lifecycle_forge()` and `project_forge()`: merge, the phase-1 PR
    /// lookups in `detect_phase1_pr`, and the refused-preflight retraction.
    /// Paths that still call `forge_for_kind` directly (`detect_merged_pr`,
    /// `ci_probe_with_forge`, the PR-opening free functions) bypass it and
    /// are out of scope for TASK-1421. With no factory this is exactly
    /// `forge_for_kind`.
    // trace:TASK-1421 | ai:claude
    pub(crate) fn forge_of_kind(
        &self,
        kind: crate::forge::ForgeKind,
    ) -> Box<dyn crate::forge::Forge> {
        match &self.forge_factory {
            Some(factory) => factory(&self.project_root, kind),
            None => crate::forge::forge_for_kind(&self.project_root, kind),
        }
    }

    /// TASK-1529: the publication guards refused, but the implementer may
    /// already have opened the PR. Retract an OPEN change through the forge so
    /// "the guards refused" and "nothing is published" are the same state. An
    /// already-merged change is left alone: closing is meaningless there and
    /// the drive's AlreadyMerged handling owns it. Best-effort: a failed
    /// close is reported and the phase still fails. Split out of
    /// `run_implementer` so a test can drive it with an injected forge.
    // trace:TASK-1529 trace:TASK-1421 | ai:codex
    pub(crate) fn retract_refused_publication(&mut self, branch: &str, detail: &str) {
        let note = implementer_preflight::retraction_notice(detail);
        // TASK-1552 finding 1: a refused change this identity could not
        // close stays OPEN on the forge — under the branch-keyed ownership
        // rule that is the COMMON rework-round case, not an edge case. Hold
        // it unmergeable instead of trusting stderr prose.
        // trace:TASK-1552 | ai:claude
        if let RetractionOutcome::LeftOpen(change) =
            self.retract_publication_with_notice(branch, &note)
        {
            self.hold_refused_publication(&change, detail);
        }
    }

    /// Says what the retraction left behind; every arm that deliberately
    /// leaves an open change on the forge (foreign or unverified ownership,
    /// or a failed close call) reports it as [`RetractionOutcome::LeftOpen`].
    pub(crate) fn retract_publication_with_notice(
        &mut self,
        branch: &str,
        note: &str,
    ) -> RetractionOutcome {
        if let Phase1PrResolve::Found(pr) = self.detect_phase1_pr(branch) {
            // A same-branch PR may have been opened by an operator while the
            // implementer was running. Branch presence alone does not prove
            // the agent published it. Require both forge identities to resolve
            // and match before applying this destructive lifecycle action.
            let forge = self.project_forge();
            let mut sink = crate::network_retry::NoopSink;
            let metadata = forge.change_metadata(pr.number, &mut sink);
            let owner = forge.authenticated_user_login();
            let ownership = match (metadata, owner) {
                (Ok(meta), Ok(owner)) if meta.state == crate::forge::ChangeState::Open => {
                    match meta.author.as_deref() {
                        Some(author) if author.eq_ignore_ascii_case(&owner) => {
                            RetractionOwnership::Ours
                        }
                        Some(_) => RetractionOwnership::Foreign,
                        None => RetractionOwnership::Unverified,
                    }
                }
                // A change that is no longer open needs no retraction, and the
                // AlreadyMerged path owns the merged case.
                (Ok(_), Ok(_)) => RetractionOwnership::NoLongerOpen,
                _ => RetractionOwnership::Unverified,
            };
            let change = crate::forge::ChangeRef {
                id: pr.number,
                url: pr.url.clone(),
                branch: pr.head_branch.clone().unwrap_or_else(|| branch.to_string()),
                base: String::new(),
                title: Some(pr.title.clone()),
            };
            match ownership {
                RetractionOwnership::Ours => {}
                RetractionOwnership::Foreign => {
                    if !self.json {
                        eprintln!(
                            "  {} left PR-{} open — this forge identity did not open it",
                            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                            pr.number,
                        );
                    }
                    return RetractionOutcome::LeftOpen(change);
                }
                RetractionOwnership::Unverified => {
                    if !self.json {
                        eprintln!(
                            "  {} PR-{} is OPEN and its publication guards FAILED, but who \
                             opened it could not be confirmed — check it by hand",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                            pr.number,
                        );
                    }
                    return RetractionOutcome::LeftOpen(change);
                }
                // trace:TASK-1552 | ai:claude
                RetractionOwnership::NoLongerOpen => {
                    return RetractionOutcome::Nothing;
                }
            }
            match forge.close_change(&change, note) {
                Ok(()) => {
                    if !self.json {
                        eprintln!(
                            "  {} closed PR-{} — it was opened before the publication \
                             guards ran, and they refused it",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            pr.number,
                        );
                    }
                    RetractionOutcome::Closed
                }
                Err(e) => {
                    if !self.json {
                        eprintln!(
                            "  {} PR-{} is OPEN and its publication guards FAILED — \
                             close it by hand: {e}",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                            pr.number,
                        );
                    }
                    RetractionOutcome::LeftOpen(change)
                }
            }
        } else {
            RetractionOutcome::Nothing
        }
    }

    /// TASK-1552 finding 1: a guards-REFUSED change that could not be closed
    /// (foreign or unverified ownership, or the close call itself failed)
    /// must not stay quietly mergeable. Stamp the typed merge-hold and its
    /// label — no ownership check, deliberately: a hold only ever TIGHTENS
    /// (the BUG-1714 doctrine) — and explain on the PR why it is held. It is
    /// released through the ordinary `aida merge-hold clear` path.
    /// Best-effort like the retraction: every miss is reported and the
    /// phase still fails.
    // trace:TASK-1552 | ai:claude
    pub(crate) fn hold_refused_publication(
        &mut self,
        change: &crate::forge::ChangeRef,
        detail: &str,
    ) {
        let number = change.id;
        let hold = crate::merge_hold::typed_hold(
            number,
            crate::merge_hold::HoldReasonKind::Supervision,
            format!(
                "publication guards refused this change and it could not be closed; \
                 it must not merge unreviewed:\n{detail}"
            ),
            None,
        )
        .with_spec(&self.spec);
        match crate::merge_hold::write_typed_hold(&self.project_root, &hold) {
            Ok(()) => {
                if !self.json {
                    eprintln!(
                        "  {} PR-{number} held unmergeable — its publication guards refused \
                         it and it could not be closed",
                        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    );
                }
            }
            Err(e) => {
                if !self.json {
                    eprintln!(
                        "  {} PR-{number} is OPEN, its publication guards REFUSED it, and \
                         the merge-hold marker could not be written — hold it by hand \
                         (`aida merge-hold add {number}`): {e}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    );
                }
            }
        }
        if let Err(err) = crate::merge_hold::sync_label(&self.project_root, number, true) {
            if !self.json {
                eprintln!(
                    "  {} merge-hold label not applied on PR-{number}: {err} — Layer 2 is off \
                     for this PR until `aida merge-hold list --fix` re-syncs it",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
        }
        let note = implementer_preflight::refused_hold_notice(detail);
        if let Err(e) = self.project_forge().comment(change, &note) {
            if !self.json {
                eprintln!(
                    "  {} the hold could not be explained on PR-{number}: {e}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                );
            }
        }
    }

    /// BUG-1690: phase 1 adopts an already-open PR as the publication to
    /// shepherd, but nothing ever un-drafted one — so a PR sitting in draft
    /// state (converted by the STORY-529 ship gate, by an operator, or opened
    /// as a draft) passed CI and review and then stalled at the phase-4 merge,
    /// which every forge refuses for a draft. Un-draft the reused PR here, at
    /// the adoption point, so a retry reaches a mergeable state. Chosen over
    /// opening a fresh PR because draft-first reuse was an operator-approved
    /// decision (see the BUG-1690 spec trail).
    ///
    /// A spec tagged `review:draft-only` is the deliberate exception: its
    /// draft IS the STORY-529 human-review hold, and the drain's merge phase
    /// never reads that tag — the draft state is what keeps the PR unmerged.
    /// Leave it, and say so.
    ///
    /// Best-effort like the retraction: a failed un-draft is reported loudly
    /// and the drive proceeds — the merge then fails with the forge's own
    /// draft refusal instead of silently stalling.
    // trace:BUG-1690 | ai:claude
    pub(crate) fn undraft_reused_publication(&mut self, pr: &OpenPrInfo) {
        let forge = self.project_forge();
        let mut sink = crate::network_retry::NoopSink;
        let Ok(meta) = forge.change_metadata(pr.number, &mut sink) else {
            // Nothing claims the PR is a draft; a wrong guess here would
            // un-draft someone else's deliberate draft. The merge's own
            // refusal remains the backstop.
            return;
        };
        if meta.state != crate::forge::ChangeState::Open || !meta.is_draft {
            return;
        }
        if self.spec_is_draft_only_tagged() {
            if !self.json {
                eprintln!(
                    "  {} PR-{} stays a draft — {} is tagged `{}`, so the draft is the \
                     human-review hold, not a stall",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    pr.number,
                    self.spec,
                    pr_ship::DRAFT_ONLY_TAG,
                );
            }
            return;
        }
        let change = crate::forge::ChangeRef {
            id: pr.number,
            url: pr.url.clone(),
            branch: pr.head_branch.clone().unwrap_or_default(),
            base: String::new(),
            title: Some(pr.title.clone()),
        };
        match forge.mark_change_ready(&change) {
            Ok(()) => {
                if !self.json {
                    eprintln!(
                        "  {} PR-{} was a draft — marked it ready so this attempt can merge it",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        pr.number,
                    );
                }
            }
            Err(e) => {
                if !self.json {
                    // The repair hint speaks the resolved forge's own syntax.
                    let repair = match forge.kind() {
                        crate::forge::ForgeKind::GitHub => format!("gh pr ready {}", pr.number),
                        crate::forge::ForgeKind::GitLab => {
                            format!("glab mr update {} --ready", pr.number)
                        }
                        crate::forge::ForgeKind::None => {
                            format!("mark change {} ready on the forge", pr.number)
                        }
                    };
                    eprintln!(
                        "  {} PR-{} is a DRAFT and could not be marked ready — the forge \
                         will refuse the merge until `{repair}` un-drafts it: {e}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        pr.number,
                    );
                }
            }
        }
    }

    /// Whether this drive's spec carries the STORY-529 `review:draft-only`
    /// tag — the one case a reused draft PR must stay a draft.
    // trace:BUG-1690 | ai:claude
    pub(crate) fn spec_is_draft_only_tagged(&self) -> bool {
        let Some(store) = load_store_for_lookup(&self.project_root) else {
            return false;
        };
        let want = self.spec.to_ascii_uppercase();
        store.requirements.iter().any(|r| {
            let matches_spec = r
                .spec_id
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case(&want))
                .unwrap_or(false)
                || r.agreed_id
                    .as_deref()
                    .map(|s| s.eq_ignore_ascii_case(&want))
                    .unwrap_or(false);
            matches_spec && {
                // The ship gate's own predicate, so the two surfaces cannot
                // drift on what counts as draft-only-tagged.
                let tags: Vec<String> = r.tags.iter().cloned().collect();
                pr_ship::is_draft_only_tagged(&tags)
            }
        })
    }

    /// BUG-1714: the publication guards came back inconclusive-only — nothing
    /// objected, nothing verified. If the implementer already opened a PR,
    /// leave it OPEN but unmergeable: stamp a typed merge-hold (plus the
    /// label) and say on the PR why it is held and that the guards will be
    /// retried. No ownership check, deliberately: a hold only ever TIGHTENS —
    /// whoever opened the PR, the work on this branch is unverified and must
    /// not merge unreviewed — and it is released through the ordinary
    /// `aida merge-hold clear` path. Best-effort like the retraction: every
    /// miss is reported and the phase still fails.
    // trace:BUG-1714 | ai:claude
    pub(crate) fn hold_inconclusive_publication(&mut self, branch: &str, detail: &str) {
        let Phase1PrResolve::Found(pr) = self.detect_phase1_pr(branch) else {
            return;
        };
        let number = pr.number;
        let hold = crate::merge_hold::typed_hold(
            number,
            crate::merge_hold::HoldReasonKind::Supervision,
            format!(
                "publication guards could not complete (inconclusive); the change is \
                 unverified and must not merge unreviewed:\n{detail}"
            ),
            None,
        )
        .with_spec(&self.spec);
        let marker_result = crate::merge_hold::write_typed_hold(&self.project_root, &hold);
        let label_result = crate::merge_hold::sync_label(&self.project_root, number, true);
        match &marker_result {
            Ok(()) => {
                if !self.json {
                    eprintln!(
                        "  {} PR-{number} left open under a merge-hold — the publication \
                         guards could not complete and will be retried",
                        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    );
                }
            }
            Err(e) => {
                if !self.json {
                    eprintln!(
                        "  {} PR-{number} is OPEN and UNVERIFIED but the merge-hold marker \
                         could not be written — hold it by hand (`aida merge-hold add \
                         {number}`): {e}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    );
                }
            }
        }
        if let Err(err) = &label_result {
            if !self.json {
                eprintln!(
                    "  {} merge-hold label not applied on PR-{number}: {err} — Layer 2 is off \
                     for this PR until `aida merge-hold list --fix` re-syncs it",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
        }
        let change = crate::forge::ChangeRef {
            id: number,
            url: pr.url.clone(),
            branch: pr.head_branch.clone().unwrap_or_else(|| branch.to_string()),
            base: String::new(),
            title: Some(pr.title.clone()),
        };
        if let (Err(marker_error), Err(label_error)) = (&marker_result, &label_result) {
            let notice = format!(
                "Publication guards could not complete, and the merge-hold could not be applied.\n\n\
                 Merge-hold marker error: {marker_error}\n\
                 Merge-hold label error: {label_error}\n\n\
                 This PR was closed only because it could not be made unmergeable. The branch is \
                 preserved, and the guards will be retried on the next publication attempt.\n\n{detail}"
            );
            if !self.json {
                eprintln!(
                    "  {} both merge-hold layers failed for PR-{number}; closing it because it could not be made unmergeable",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
            if !matches!(
                self.retract_publication_with_notice(branch, &notice),
                RetractionOutcome::Closed
            ) {
                // The PR could be neither held nor closed. The last honest
                // lever is a comment: put the warning ON the PR so whoever
                // can merge it sees that the work is unverified.
                let warning = format!(
                    "Publication guards could not complete, and the merge-hold could not \
                     be applied.\n\n\
                     Merge-hold marker error: {marker_error}\n\
                     Merge-hold label error: {label_error}\n\n\
                     This PR could be neither held nor closed: it is OPEN and UNVERIFIED \
                     and must not merge unreviewed. The guards will be retried on the \
                     next publication attempt.\n\n{detail}"
                );
                if let Err(e) = self.project_forge().comment(&change, &warning) {
                    if !self.json {
                        eprintln!(
                            "  {} PR-{number} is OPEN, UNVERIFIED, and carries no hold \
                             layer — could not even warn on it: {e}",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        );
                    }
                }
            }
            return;
        }
        let note = implementer_preflight::inconclusive_hold_notice(detail);
        if let Err(e) = self.project_forge().comment(&change, &note) {
            if !self.json {
                eprintln!(
                    "  {} could not explain the hold on PR-{number}: {e}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                );
            }
        }
    }

    pub(crate) fn lifecycle_forge(&self) -> Box<dyn crate::forge::Forge> {
        self.forge_of_kind(self.lifecycle_forge)
    }

    /// The project's configured forge — the driver-side equivalent of
    /// `forge_for(&self.project_root)`, routed through the injection seam.
    // trace:TASK-1421 | ai:claude
    pub(crate) fn project_forge(&self) -> Box<dyn crate::forge::Forge> {
        self.forge_of_kind(crate::forge::resolve_forge_kind(&self.project_root))
    }

    pub(crate) fn record_auto_rebase(&mut self, pr_number: u64, outcome: impl Into<String>) {
        self.auto_rebase_events
            .push(auto_complete_telemetry::AutoRebaseEvent {
                phase: auto_complete::Phase::Reviewer.index() as u8,
                pr_number,
                outcome: outcome.into(),
            });
    }

    pub(crate) fn should_auto_rebase_stale_base(
        &self,
        attempted: bool,
    ) -> Result<(), &'static str> {
        if self.allow_stale_base {
            return Err("allow-stale-base");
        }
        if self.no_auto_rebase {
            return Err("disabled");
        }
        // BUG-1268: reviewer-only headless runs have nobody available to
        // perform the mechanical recovery either. The safety boundary is a
        // headless reviewer, not whether phase 1 was also headless.
        // trace:BUG-1268 | ai:codex
        if self.no_human.is_none() {
            return Err("not-headless");
        }
        if attempted {
            return Err("retry-limit");
        }
        Ok(())
    }

    pub(crate) fn attempt_phase3_auto_rebase(
        &mut self,
        pr_number: u64,
    ) -> Result<(), auto_complete::PhaseFailure> {
        // BUG-1574: this is the concrete force-push path an unattended drain
        // takes on the spec's OWN branch (`aida pr rebase` → BUG-640's
        // anchored-lease push). Refuse BEFORE spawning it — never crash —
        // when the spec is keyboard-only or outside the currently active
        // batch's declared member set. Fail-closed: a declared-but-broken
        // store or malformed drain-state.json also refuses. Typed
        // `StaleBaseRefused` (not a generic `Failed`) because this refusal
        // has the exact same shape and handling contract as the OTHER
        // force-push refusal below (BUG-1218: a non-transient, operator-
        // reconciliation shelve — never worth a retry, which would
        // deterministically hit the same guard again); the caller's
        // shelve-and-continue path (resilient-drain / STORY-281) reads that
        // typed kind, not this prose. trace:BUG-1574 | ai:claude
        if let Some(reason) = unattended_git_mutation_refusal(
            &self.project_root,
            &self.spec,
            "rebase/force-push",
            self.branch.as_deref(),
        ) {
            if !self.json {
                eprintln!(
                    "  {} auto-rebase of PR-{pr_number} refused: {reason}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
                );
            }
            self.record_auto_rebase(pr_number, "refused-scope-guard");
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::StaleBaseRefused,
                reason,
            ));
        }
        // (classification helper `classify_rebase_subprocess_exit` lives at
        // crate scope — see below — so it's unit-testable without spawning
        // a subprocess. trace:BUG-1295 | ai:claude)
        if !self.json {
            eprintln!(
                "  {} stale-base + overlap detected on PR-{pr_number}; attempting one clean auto-rebase…",
                "↻".cyan()
            );
        }
        let status = std::process::Command::new(self.aida_exe())
            .current_dir(&self.project_root)
            .args(build_phase3_auto_rebase_args(pr_number))
            .output_retrying_etxtbsy();
        match status {
            Ok(output) if output.status.success() => {
                if !output.stdout.is_empty() {
                    eprint!("{}", String::from_utf8_lossy(&output.stdout));
                }
                if !output.stderr.is_empty() {
                    eprint!("{}", String::from_utf8_lossy(&output.stderr));
                }
                self.record_auto_rebase(pr_number, "clean");
                if !self.json {
                    eprintln!(
                        "  {} PR-{pr_number} auto-rebased cleanly; proceeding with phase 3",
                        crate::glyph(crate::glyphs::Glyph::Check).green().bold()
                    );
                }
                Ok(())
            }
            Ok(output) => {
                let detail = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                if !detail.is_empty() {
                    eprint!("{detail}");
                }
                // BUG-1295: classify on the subprocess's EXIT CODE via the
                // pure `classify_rebase_subprocess_exit` helper, not by
                // substring-matching its stdout/stderr prose. The six bail
                // sites this used to match (pr_cmd.rs conflict/force-push
                // bails, rebase_cmd.rs, pr_rebase.rs) could be reworded
                // independently and silently fall through to the untyped
                // arm below — a conflict losing `StaleBaseConflict` (and the
                // `--interactive` recovery recipe attached to it) or a
                // safety refusal being reclassified as a generic failure,
                // with nothing to catch the drift. `pr_cmd.rs`'s conflict
                // and force-push-refused bails now return
                // `pr_rebase::rebase_conflict_error` / `rebase_refused_error`,
                // which `main_entry` downcasts to pick
                // `pr_rebase::REBASE_EXIT_CODE_CONFLICT` /
                // `REBASE_EXIT_CODE_REFUSED` as this process's exit code —
                // the one signal both ends read from the same shared
                // constant. See
                // `tests/bug_1295_rebase_exit_code_tests.rs` for the drift
                // guard. trace:BUG-1295 | ai:claude
                match classify_rebase_subprocess_exit(output.status.code()) {
                    Some(auto_complete::FailureKind::StaleBaseRefused) => {
                        self.record_auto_rebase(pr_number, "stale-base-refused");
                        return Err(auto_complete::PhaseFailure::of(
                            auto_complete::FailureKind::StaleBaseRefused,
                            detail.trim().to_string(),
                        ));
                    }
                    Some(auto_complete::FailureKind::StaleBaseConflict) => {
                        self.record_auto_rebase(pr_number, "conflict");
                        let recipe = format!(
                            "{}\n\nManual recovery: `aida pr rebase {pr_number} --interactive`",
                            detail.trim()
                        );
                        return Err(auto_complete::PhaseFailure::of(
                            auto_complete::FailureKind::StaleBaseConflict,
                            recipe,
                        ));
                    }
                    _ => {}
                }
                self.record_auto_rebase(pr_number, "failed");
                Err(auto_complete::PhaseFailure::new(detail.trim().to_string()))
            }
            Err(e) => {
                self.record_auto_rebase(pr_number, format!("failed:{e}"));
                Err(auto_complete::PhaseFailure::new(format!(
                    "auto-rebase failed to start: {e}"
                )))
            }
        }
    }

    pub(crate) fn resolve_phase3_stale_overlap(
        &mut self,
        pr_number: u32,
        auto_rebase_attempted: &mut bool,
        msg: String,
    ) -> Phase3StaleOverlapAction {
        match self.should_auto_rebase_stale_base(*auto_rebase_attempted) {
            Ok(()) => {
                *auto_rebase_attempted = true;
                match self.attempt_phase3_auto_rebase(pr_number as u64) {
                    Ok(()) => return Phase3StaleOverlapAction::Proceed,
                    Err(failure)
                        if matches!(
                            failure.kind,
                            auto_complete::FailureKind::StaleBaseRefused
                                | auto_complete::FailureKind::StaleBaseConflict
                        ) =>
                    {
                        return Phase3StaleOverlapAction::Refuse(failure);
                    }
                    Err(_) => {}
                }
                Phase3StaleOverlapAction::Refuse(auto_complete::PhaseFailure::new(msg))
            }
            Err("allow-stale-base") => {
                self.record_auto_rebase(pr_number as u64, "skipped:allow-stale-base");
                eprintln!(
                    "  {} stale-base + overlap detected; `--allow-stale-base` is set, \
                     proceeding with reviewer.\n{}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                    msg.yellow()
                );
                Phase3StaleOverlapAction::Proceed
            }
            Err(reason) => {
                self.record_auto_rebase(pr_number as u64, format!("skipped:{reason}"));
                Phase3StaleOverlapAction::Refuse(auto_complete::PhaseFailure::new(msg))
            }
        }
    }

    pub(crate) fn auto_punt_text_question(
        &self,
        worktree_path: &std::path::Path,
        session_uuid: &str,
    ) -> Option<String> {
        if !self
            .no_human
            .map(|m| m.wants_headless_implementer())
            .unwrap_or(false)
        {
            return None;
        }
        let content = read_headless_log_for_session(&self.project_root, session_uuid)?;
        let punt = pending_text_question_from_headless_log(&content)?;
        let status = std::process::Command::new(self.aida_exe())
            .current_dir(worktree_path)
            .args(build_auto_punt_args(&self.spec, &punt.detail, &punt.lean))
            .env("AIDA_SESSION_ROLE", "implementer")
            .status_retrying_etxtbsy()
            .ok()?;
        if !status.success() {
            return None;
        }
        Some(format!(
            "[design-fork] {} (lean: {})",
            punt.detail, punt.lean
        ))
    }

    /// Locate the session lease `aida queue work --session-id <uuid>` created
    /// for this orchestrator phase, pinned by the `claude_session_id` the
    /// orchestrator minted. Deterministic — unaffected by concurrent leases
    /// from parallel user sessions or nested `/aida-pickup`, and immune to
    /// the `.manifest.toml` companion file that the old lease-set diff
    /// miscounted as a second lease (the BUG-114 phase-1 failure). On a miss,
    /// the error suggests bare `--resume` (continues the most recent recorded
    /// claude session) and lists the candidate lease ids as diagnostic-only —
    /// they are lease ids, not `--resume` arguments (TASK-271).
    /// trace:BUG-114 trace:TASK-271 | ai:claude
    pub(crate) fn discover_orchestrated_lease(
        &self,
        claude_session_id: &str,
    ) -> Result<(String, String, std::path::PathBuf), auto_complete::PhaseFailure> {
        find_orchestrated_lease(&self.project_root, claude_session_id)
            .or_else(|| {
                read_orchestrated_lease_receipt(
                    &orchestrated_lease_receipt_path(&self.project_root, claude_session_id),
                    claude_session_id,
                )
            })
            .map(|(id, branch, worktree, _)| (id, branch, worktree))
            .ok_or_else(|| {
                let candidates = lease_ids_in(&self.sessions_dir());
                if candidates.is_empty() {
                    // BUG-1524: no lease, no receipt, no other candidate in
                    // flight — the child never got far enough to mint
                    // anything, i.e. it never launched an implementer
                    // session at all (a preflight/lease refusal or an
                    // agent-binary launch failure). Typed as
                    // `LaunchRefused` so the orchestrator never treats an
                    // already-open PR from a previous round as evidence
                    // this round did anything. trace:BUG-1524 | ai:claude
                    auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::LaunchRefused,
                        "no session lease appeared — `aida queue work` did not start a session",
                    )
                } else if let Some(conflict_failure) =
                    scope_conflict_lease_failure(&self.project_root, &self.spec)
                {
                    // BUG-1285: the dominant real-world cause of "no lease
                    // minted under this run's session id" is a same-scope
                    // lease conflict — the child `aida queue work` claim gate
                    // refused before it ever got to mint one. Name the
                    // holding lease + its liveness, not a bare id list the
                    // operator has to cross-reference by hand — and route it
                    // through `FailureKind::LeaseConflict`, which is NOT a
                    // transient-retry cause, so a refusal that IS correct
                    // (the holder is still live) does not burn the drain's
                    // retry budget rediscovering the same conflict. This is
                    // also a launch refusal (the claim gate blocked the
                    // child before it started). It keeps its own kind; the
                    // orchestrator skips open-PR recovery for it exactly as
                    // for `LaunchRefused` (BUG-1524).
                    // trace:BUG-1285 | ai:claude
                    conflict_failure
                } else {
                    // BUG-1524: candidates exist (other leases are live),
                    // but none of them is THIS session's — and no BUG-1485
                    // handoff receipt exists either, so this session did
                    // not even reach the point of completing genuine work
                    // and releasing its lease cleanly. Typed the same way
                    // as the empty-candidates case above.
                    // trace:BUG-1485 | ai:codex trace:BUG-1524 | ai:claude
                    // BUG-1629: no lease and no receipt means the child
                    // never started an implementer session, so there is no
                    // session to `--resume`; say so instead of suggesting it.
                    // trace:BUG-1629 | ai:claude
                    auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::LaunchRefused,
                        format!(
                            "the child session {} neither retained its session lease nor wrote its \
                             orchestrator handoff receipt, so no implementer session started and \
                             there is nothing to resume; unrelated active leases were ignored.",
                            &claude_session_id[..claude_session_id.len().min(8)],
                        ),
                    )
                }
            })
    }

    /// BUG-1629: the workspace this phase-1 launch uses. A BUG-908 retry pin
    /// wins (the substrate re-targets the predecessor's worktree); otherwise
    /// the workspace the orchestrator checked and pinned. A direct caller that
    /// skipped the orchestrator check resolves exactly once and pins the
    /// result. A resolver failure refuses the launch: the implementer is never
    /// launched without a pinned worktree (BUG-1244).
    ///
    /// The first launch of a freshly resolved branch also closes the explicit
    /// `--branch` reuse race. `queue work --branch X` checks out `X` when it
    /// already exists (the TASK-245 fixup rule), while an ordinary pickup only
    /// ever picks a free name. A branch that was free when phase 1 resolved it
    /// but exists at launch was created by someone else in between, so the
    /// launch is refused instead of adopting it. Retries, and workspaces taken
    /// from this spec's own lease, keep the TASK-245 reuse: that branch is
    /// this spec's own.
    // trace:BUG-1629 | ai:claude
    pub(crate) fn phase1_launch_workspace(
        &mut self,
    ) -> Result<(std::path::PathBuf, String), auto_complete::PhaseFailure> {
        if let (Some(worktree), Some(branch)) = (
            &self.retry_implementer_worktree,
            &self.retry_implementer_branch,
        ) {
            return Ok((worktree.clone(), branch.clone()));
        }
        if self.phase1_workspace.is_none() {
            match auto_complete::PhaseDriver::implementer_workspace(self) {
                Ok(Some((worktree, branch))) => {
                    auto_complete::PhaseDriver::pin_implementer_workspace(self, &worktree, &branch)
                }
                Ok(None) => {
                    return Err(auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::LaunchRefused,
                        auto_complete::phase1_resolver_failure_reason(
                            &self.spec,
                            "no workspace was resolved",
                        ),
                    ))
                }
                Err(err) => {
                    return Err(auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::LaunchRefused,
                        auto_complete::phase1_resolver_failure_reason(&self.spec, &err),
                    ))
                }
            }
        }
        let project_root = self.project_root.clone();
        let spec = self.spec.clone();
        let pin = self
            .phase1_workspace
            .as_mut()
            .expect("phase-1 workspace pinned above");
        if pin.fresh {
            let exists_now = branch_exists_anywhere(&project_root, &pin.branch);
            if phase1_fresh_branch_raced(pin.fresh, exists_now) {
                return Err(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::LaunchRefused,
                    phase1_branch_race_reason(&spec, &pin.branch),
                ));
            }
            pin.fresh = false;
        }
        Ok((pin.worktree.clone(), pin.branch.clone()))
    }

    /// BUG-1629: a phase-1 child exited without its session lease and without
    /// its orchestrator handoff receipt, so no implementer session started
    /// (nothing ran, nothing to resume). Leave no lease behind, then launch
    /// one clean replacement. When the pinned worktree already exists on the
    /// pinned branch with no lease holding it, the replacement re-enters it
    /// through the BUG-908 retry pin; otherwise it launches fresh on the same
    /// pinned workspace. A second loss is final: the typed `LaunchRefused`
    /// failure shelves the spec, so dependents stay blocked.
    ///
    /// Launch budget: this replacement is the only launch this method adds.
    /// Each launch (first or replacement) can still spend the separate
    /// BUG-826 zero-byte-log budget (two relaunches in total per phase 1
    /// run), so the total number of child launches can exceed two. Both
    /// counters live on the driver and are never reset, so the total stays
    /// bounded.
    // trace:BUG-1629 | ai:claude
    pub(crate) fn recover_lost_child_state(
        &mut self,
        session_uuid: &str,
        exit: &str,
        elapsed: std::time::Duration,
    ) -> Result<auto_complete::ImplementerOutcome, auto_complete::PhaseFailure> {
        // No orphan: release any lease keyed to this session id (a no-op when
        // the child minted none) and drop a partial receipt.
        self.release_empty_launch_lease(session_uuid);
        let _ = std::fs::remove_file(orchestrated_lease_receipt_path(
            &self.project_root,
            session_uuid,
        ));
        // The child's own refusal (a preflight/lease refusal printed on its
        // stderr), when `queue work` recorded one for this session.
        let refusal = take_orchestrated_child_refusal(&self.project_root, session_uuid);
        let mut state = lost_child_state_reason(session_uuid, exit, elapsed);
        if let Some(refusal) = &refusal {
            state.push_str(&format!("; the child refused: {refusal}"));
        }
        // BUG-1769: this is the arbiter BUG-1140 mandated, and until now it
        // consulted no substrate signal at all. Its whole decision input was
        // lease-absent + receipt-absent + the retry budget — which cannot see
        // the case BUG-1140 describes: a child that committed, pushed, advanced
        // the spec to Done and then exited without its lease or receipt. For
        // BUG-1817 that cost a replacement launch the child's own claim gate
        // refused with "Status is Done (work finished on a branch)" — a success
        // report — and then demoted a finished spec to NeedsAttention.
        // trace:BUG-1769 trace:BUG-1140 | ai:claude
        let current_status = spec_status(&self.project_root, &self.spec);
        let entry_status = self
            .phase1_entry_spec_status
            .as_ref()
            .and_then(|status| status.as_ref());
        if phase1_advanced_to_finished_during_run(entry_status, current_status.as_ref()) {
            return self.resolve_phase1_finished_without_lease(&state, current_status.as_ref());
        }
        if !phase1_lost_child_retry_allowed(self.lost_child_state_retries_used) {
            let how = if refusal.is_some() {
                "was refused"
            } else {
                "lost its state the same way"
            };
            // BUG-1769: this message used to assert "<spec> was not advanced"
            // with nothing behind it, and said exactly that about a run that
            // HAD advanced the spec to Done. The no-advance claim is now
            // scoped to the replacement launch this method spent, and the
            // status it reports is the `spec_status` read above.
            //
            // The elapsed time in `{state}` is the replacement child's OWN
            // lifetime. Phase 1's recorded `phase_durations` entry covers every
            // launch in the phase — BUG-1817 recorded 426 837 ms for a phase
            // whose blamed child lived 302 ms — so the message says which child
            // the elapsed time belongs to rather than leaving the reader to
            // attribute the phase total to it. trace:BUG-1769 | ai:claude
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::LaunchRefused,
                format!(
                    "{state}. The single clean replacement launch already ran and {how}; no \
                     session lease was left behind, and that replacement launch did not \
                     advance {spec} — the store reports {spec} as {status}. The elapsed time \
                     above is that replacement child's own lifetime; phase 1's recorded \
                     duration covers every launch in the phase, not this child alone",
                    spec = self.spec,
                    status = phase1_spec_status_text(current_status.as_ref()),
                ),
            ));
        }
        self.lost_child_state_retries_used += 1;
        if self.retry_implementer_worktree.is_none() {
            if let Some(pin) = &self.phase1_workspace {
                let lease_worktrees: Vec<std::path::PathBuf> = list_leases(&self.project_root)
                    .into_iter()
                    .map(|lease| lease.worktree_path)
                    .collect();
                if pin.worktree.exists()
                    && current_branch_at(&pin.worktree).as_deref() == Some(pin.branch.as_str())
                    && worktree_is_unleased(&pin.worktree, &lease_worktrees)
                {
                    self.retry_implementer_worktree = Some(pin.worktree.clone());
                    self.retry_implementer_branch = Some(pin.branch.clone());
                }
            }
        }
        self.record_lost_child_replacement(&state);
        auto_complete::PhaseDriver::run_implementer(self)
    }

    /// BUG-1769: read the spec's store status once, at phase-1 entry. Later
    /// calls are no-ops: `run_implementer` is re-entered by the BUG-1629
    /// replacement launch and the BUG-826 zero-byte-log relaunches, and a
    /// second capture would replace the entry reading with a mid-run one, which
    /// is precisely the comparison this field exists to make.
    // trace:BUG-1769 | ai:claude
    pub(crate) fn capture_phase1_entry_spec_status(&mut self) {
        if self.phase1_entry_spec_status.is_none() {
            self.phase1_entry_spec_status = Some(spec_status(&self.project_root, &self.spec));
        }
    }

    /// BUG-1769: phase 1's child lost its lease and its handoff receipt, but
    /// the substrate says THIS run finished the work — the spec advanced to
    /// `Done` (or beyond) from an entry status that was not finished. Spending
    /// the BUG-1629 replacement launch here is pure waste: the child's own
    /// claim gate refuses an already-`Done` spec, and its refusal text ends
    /// "nothing to do — auto-bump fires when the PR merges", which is a success
    /// report. Typing that refusal `LaunchRefused` is what demoted BUG-1817
    /// from `Done` to `NeedsAttention` and auto-drafted a non-bug for a human.
    ///
    /// So resolve it the way a lease-bearing child's work is resolved, against
    /// the pinned phase-1 branch: an open PR means `PrOpened` and the run
    /// continues into CI + review (the real quality gates), a merged one means
    /// `AlreadyMerged` (BUG-709 — the implementer ran the full ship itself),
    /// and committed-but-unopened work is recovered into a PR (BUG-893). This
    /// is the `ImplementerOutcome::AlreadyMerged` shape applied to a second
    /// instance of the same defect class: the work succeeded, the orchestrator
    /// looked for its customary artifact, did not find it, and false-negatived
    /// a finished drive.
    ///
    /// Only when no PR can be found OR opened is the result `Inconclusive`: the
    /// work is real but its artifact is not locatable, so the drain pauses and
    /// leaves the spec where it is. That is deliberately not a failure — the
    /// one thing this path must never do is demote a spec this run finished.
    // trace:BUG-1769 | ai:claude
    pub(crate) fn resolve_phase1_finished_without_lease(
        &mut self,
        state: &str,
        current_status: Option<&RequirementStatus>,
    ) -> Result<auto_complete::ImplementerOutcome, auto_complete::PhaseFailure> {
        let status = phase1_spec_status_text(current_status);
        let Some((worktree, branch)) = self
            .phase1_workspace
            .as_ref()
            .map(|pin| (pin.worktree.clone(), pin.branch.clone()))
        else {
            return Ok(auto_complete::ImplementerOutcome::Inconclusive {
                reason: format!(
                    "{state}; but the store reports {} as {status}, so this run finished the \
                     work — no replacement launch was spent and nothing was shelved. No \
                     phase-1 workspace was pinned, so there is no branch to look a PR up on",
                    self.spec
                ),
                retry_hint: None,
            });
        };
        if !self.json {
            eprintln!(
                "  {} {} is {} — this run finished the work, so the lost child's missing lease \
                 is not a failure; resolving the PR on `{}` instead of relaunching",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                self.spec,
                status,
                branch,
            );
        }
        self.implementer_worktree = Some(worktree.clone());
        self.branch = Some(branch.clone());
        match self.detect_phase1_pr(&branch) {
            Phase1PrResolve::Found(pr) => {
                self.set_pr_number(pr.number as u32);
                // Same adoption point as the main phase-1 verify: a reused
                // draft PR must be un-drafted or the merge is refused.
                // trace:BUG-1690 | ai:claude
                self.undraft_reused_publication(&pr);
                if !self.json {
                    eprintln!(
                        "  {} PR-{} found on `{}` — continuing into CI + review",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        pr.number,
                        branch,
                    );
                }
                Ok(auto_complete::ImplementerOutcome::PrOpened)
            }
            // BUG-709: the implementer ran the full ship itself, so there is no
            // open PR to shepherd and nothing to tear down (this child never
            // took a lease). trace:BUG-709 | ai:claude
            Phase1PrResolve::AlreadyMerged(pr) => {
                self.set_pr_number(pr.number as u32);
                if !self.json {
                    eprintln!(
                        "  {} PR-{} already merged — completing the drive",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        pr.number,
                    );
                }
                Ok(auto_complete::ImplementerOutcome::AlreadyMerged {
                    pr_number: pr.number as u32,
                })
            }
            // No open PR, or the lookup could not reach the forge. Either way
            // the commits may be sitting in the pinned worktree unopened —
            // BUG-893's recovery is the same one the lease-bearing path runs.
            // trace:BUG-893 | ai:claude
            other => {
                if let Some((ahead, pr)) = try_open_orchestrator_pr_for_no_pr_worktree(
                    &self.project_root,
                    &worktree,
                    &branch,
                    self.lifecycle_forge,
                    &self.spec,
                ) {
                    if !self.json {
                        eprintln!(
                            "  {} {} commit(s) on `{}` with no PR — opened PR-{} for them \
                             (BUG-893 recovery)",
                            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                            ahead,
                            branch,
                            pr,
                        );
                    }
                    self.set_pr_number(pr as u32);
                    return Ok(auto_complete::ImplementerOutcome::PrOpened);
                }
                let why = match other {
                    Phase1PrResolve::NoPr => "no open PR on the branch".to_string(),
                    Phase1PrResolve::Fail(f) => f.reason,
                    Phase1PrResolve::Retry(reason) => reason,
                    Phase1PrResolve::Found(_) | Phase1PrResolve::AlreadyMerged(_) => {
                        unreachable!("resolved above")
                    }
                };
                Ok(auto_complete::ImplementerOutcome::Inconclusive {
                    reason: format!(
                        "{state}; but the store reports {} as {status}, so this run finished \
                         the work — no replacement launch was spent and nothing was shelved. \
                         No PR could be found or opened for `{branch}`: {why}",
                        self.spec
                    ),
                    retry_hint: None,
                })
            }
        }
    }

    /// BUG-1629: leave a durable trace of the replacement launch — the drain
    /// retry ledger, a `SpecRetried` event (cause `lost-child-state`), and a
    /// `retrying` line under `--json` — the same channels
    /// `record_transient_retry` uses, without its model escalation.
    // trace:BUG-1629 | ai:claude
    pub(crate) fn record_lost_child_replacement(&self, state: &str) {
        let phase = auto_complete::Phase::Implementer;
        drain_state::append_phase_retry(
            &self.project_root,
            &self.spec,
            &format!("{} ({})", phase.index(), phase.slug()),
            PHASE1_LOST_CHILD_STATE_CAUSE,
            2,
            2,
        );
        let (_, run_uuid) = drain_state::current_context(&self.project_root);
        events::emit(
            &self.project_root,
            &events::Event::new(
                Some(self.spec.clone()),
                run_uuid,
                events::EventKind::SpecRetried {
                    phase: phase.slug().to_string(),
                    cause: PHASE1_LOST_CHILD_STATE_CAUSE.to_string(),
                    attempt: 2,
                    max: 2,
                    model_before: None,
                    model_after: None,
                    detail: Some(state.to_string()),
                },
            ),
        );
        if self.json {
            println!(
                "{}",
                auto_complete::phase_event(
                    phase.slug(),
                    "retrying",
                    &self.spec,
                    0,
                    None,
                    &[
                        ("kind", PHASE1_LOST_CHILD_STATE_CAUSE),
                        ("attempt", "2"),
                        ("max", "2"),
                        ("detail", state),
                    ],
                )
            );
        } else {
            eprintln!(
                "  {} {state}; launching one clean replacement (attempt 2/2)",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            );
        }
    }

    /// BUG-826: after an empty-log phase-1 vendor death, release only the lease
    /// minted for that failed session UUID so the next retry starts clean.
    // trace:BUG-826 | ai:codex
    pub(crate) fn release_empty_launch_lease(&mut self, session_uuid: &str) {
        let Some((lease_id, _branch, worktree, _)) =
            find_orchestrated_lease(&self.project_root, session_uuid)
        else {
            return;
        };
        self.implementer_lease = Some(lease_id.clone());
        self.implementer_worktree = Some(worktree);
        if let Err(e) = self.end_implementer_session() {
            if !self.json {
                eprintln!(
                    "  {} empty-log launch left lease {}, but auto-release failed ({}) - \
                     the next retry may need `--force-claim`",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    &lease_id[..lease_id.len().min(8)],
                    e.reason,
                );
            }
        } else {
            // BUG-1716: the lease is gone — forget it. Leaving the field set
            // made the TASK-133 compensation gate read "lease acquired ⇒ work
            // may exist" and skip the pre-bump status restore, stranding the
            // never-started spec NeedsAttention (blocked from the safe retry
            // as `needs-triage`) — the exact demotion this empty-launch lane
            // exists to prevent. On a failed auto-release the fields stay set
            // on purpose: a real lease survives on disk, and the BUG-479
            // probe should keep refusing the restore for it.
            // trace:BUG-1716 | ai:claude
            self.implementer_lease = None;
            self.implementer_worktree = None;
            if !self.json {
                eprintln!(
                    "  {} released empty-log launch lease {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    &lease_id[..lease_id.len().min(8)],
                );
            }
        }
    }

    /// Ground-truth check for the BUG-241 reconcile: find a *merged* PR that
    /// credits the dispatched spec. BUG-357 makes the spec-credit check
    /// mandatory: an unrelated merged PR on the same/misattributed branch is
    /// not evidence that this spec shipped. Prefers the PR number an earlier
    /// phase already discovered (checked directly with
    /// [`pr_is_merged_with_sink`]); otherwise looks one up by branch.
    /// `None` when no merged PR exists, the PR credits a different spec, or
    /// `gh` can't answer — all leave the original failure standing.
    /// trace:BUG-241, BUG-286, BUG-357 | ai:claude
    pub(crate) fn detect_merged_pr(&self) -> Option<u32> {
        // BUG-286: route the orchestrator's reconcile-time `gh pr view`
        // through the retry helper with stderr + drain-state sinks. A
        // transient blip in this read used to leave the failure standing
        // (returning `None` is "cannot confirm", which leaves reconcile_failure
        // at GenuineFailure). Retry first, surface the retry to both channels.
        // trace:BUG-286 | ai:claude
        let mut stderr_sink = network_retry::StderrSink;
        let mut state_sink = drain_state::DrainStateSink {
            project_root: &self.project_root,
            spec: self.spec.clone(),
            phase: Some(format!(
                "{} (reconcile)",
                auto_complete::Phase::Reviewer.index()
            )),
        };
        let mut sink = network_retry::DualSink {
            a: &mut stderr_sink,
            b: &mut state_sink,
        };
        let latest_reopen_at = spec_latest_reopen_transition_at(&self.project_root, &self.spec);
        if let Some(pr) = self.pr_number {
            // We already know the PR — `gh pr view` is the direct check.
            if pr_is_merged_with_sink(&self.project_root, pr, &mut sink) != Some(true) {
                return None;
            }
            if !self.merged_pr_satisfies_latest_reopen(pr, latest_reopen_at, &mut sink) {
                return None;
            }
            return matches!(
                pr_credit_match_with_sink(&self.project_root, pr, &self.spec, None, &mut sink),
                PrCreditMatch::Dispatched
            )
            .then_some(pr);
        }
        // Phase 1 never resolved a PR — try a branch-keyed merged-PR lookup.
        // `detect_merged_pr_for_branch` already classifies network errors
        // into `PrLookup::GhUnreachable` and the orchestrator already absorbs
        // them as "cannot confirm" — so the branch leg stays as-is.
        match self
            .branch
            .as_deref()
            .map(|b| detect_merged_pr_for_branch_via_forge(&self.project_root, b))
        {
            Some(PrLookup::Found(pr)) => {
                if !self.merged_pr_satisfies_latest_reopen(
                    pr.number as u32,
                    latest_reopen_at,
                    &mut sink,
                ) {
                    return None;
                }
                matches!(
                    pr_credit_match_with_sink(
                        &self.project_root,
                        pr.number as u32,
                        &self.spec,
                        Some(&pr.title),
                        &mut sink,
                    ),
                    PrCreditMatch::Dispatched
                )
                .then_some(pr.number as u32)
            }
            _ => None,
        }
    }

    // trace:BUG-1112 | ai:codex
    pub(crate) fn merged_pr_satisfies_latest_reopen(
        &self,
        pr: u32,
        latest_reopen_at: Option<chrono::DateTime<chrono::Utc>>,
        sink: &mut dyn network_retry::RetrySink,
    ) -> bool {
        if latest_reopen_at.is_none() {
            return true;
        }
        let metadata = self.project_forge().change_metadata(pr as u64, sink).ok();
        let merged_at = metadata
            .filter(|m| m.state == crate::forge::ChangeState::Merged)
            .and_then(|m| m.merged_at);
        merged_at_satisfies_latest_reopen(merged_at, latest_reopen_at)
    }

    /// STORY-301: stamp the drain-state file with the phase about to run, so
    /// `aida drain status` (and the `/aida-pickup` banner) show live progress.
    /// Best-effort — a missing file is a silent no-op, never blocks the phase.
    ///
    /// TASK-1292: also carries the run's frozen PR binding (`phase_done_pr`,
    /// falling back to the raw `pr_number` before phase 1 has frozen it) so
    /// the member's `pr` field is live the moment a phase starts, not only
    /// once it finishes. This is what makes `aida pr ship`'s drive-ownership
    /// check PR-keyed instead of spec-keyed — see
    /// `pr_ship::reviewer_liveness_for_pr`.
    // trace:STORY-301 | ai:claude
    // trace:TASK-1292 | ai:claude
    pub(crate) fn mark_drain_phase(&self, phase: auto_complete::Phase) {
        let vendor = session::resolve_headless_vendor(&self.project_root);
        let seat = agent_seat_for_phase(phase);
        let tuning = session::resolve_agent_tuning(&self.project_root, vendor, seat);
        drain_state::set_phase_with_tuning(
            &self.project_root,
            &self.spec,
            phase.index(),
            phase.slug(),
            Some(vendor.as_str()),
            Some(seat.as_str()),
            tuning.model.as_deref(),
            tuning.effort.as_deref(),
            self.phase_done_pr.or(self.pr_number),
        );
    }

    /// BUG-872: stamp the phase with the exact headless session id whose log
    /// should be treated as current. Status/tail readers must not select an
    /// older retry's log for the same spec while the fresh phase is starting.
    ///
    /// BUG-1290: `announce` must be `true` ONLY when this call is the phase's
    /// sole entry point (no preceding `mark_drain_phase` call in this same
    /// function — today that's just `run_implementer`, phase 1). Every other
    /// caller already announced the phase via `mark_drain_phase` and must
    /// pass `false` so this call attaches the session without re-emitting a
    /// second `PhaseEntered` for the same entry — the duplicate this spec
    /// fixes.
    // trace:BUG-872 trace:BUG-1290 | ai:claude
    pub(crate) fn mark_drain_phase_session(
        &self,
        phase: auto_complete::Phase,
        session_id: &str,
        vendor: session::HeadlessVendor,
        announce: bool,
    ) {
        drain_state::set_phase_session_vendor(
            &self.project_root,
            &self.spec,
            phase.index(),
            phase.slug(),
            session_id,
            vendor,
            announce,
        );
    }

    /// STORY-347: spawn one headless advisor (cold-boot OR fork) against the
    /// already-written punt request and collect its [`punt::PuntResponse`] +
    /// JSONL log path. Factored out so the calibration loop can run two
    /// passes against the same request — one cold-boot driving the drain,
    /// one fork-from-live as shadow. Returns the response and absolute log
    /// path; an error is a phase-fatal (the spawn failed or the advisor
    /// wrote no usable response).
    ///
    /// `response_path` is the file the advisor will write to — distinct
    /// paths for the primary and shadow passes so they don't clobber. The
    /// caller is responsible for clearing any stale file beforehand.
    /// trace:STORY-347 | ai:claude
    pub(crate) fn spawn_advisor_session(
        &self,
        pass: AdvisorPass,
        advisor_cfg: &advisor::AdvisorConfig,
        response_path: &std::path::Path,
        request_path: &std::path::Path,
    ) -> Result<(punt::PuntResponse, std::path::PathBuf), auto_complete::PhaseFailure> {
        use auto_complete::{FailureKind, PhaseFailure};

        // TASK-894: resolve the vendor that hosts the advisor tier. Default is
        // Claude — selecting Codex (via `AIDA_HEADLESS_VENDOR=codex` or
        // `[orchestrator] headless_vendor = "codex"`) is the explicit opt-in,
        // mirroring STORY-683's drain-phase generalization. A non-Claude vendor
        // has no `--resume` / session model, so the fork-from-live JSONL
        // machinery (Claude-specific) must not run; force the pass to cold-boot
        // and host a fresh spawn per punt. trace:TASK-894 | ai:claude
        let vendor = session::resolve_headless_vendor(&self.project_root);
        let pass = match pass {
            AdvisorPass::Fork(_) if vendor != session::HeadlessVendor::Claude => {
                if !self.json {
                    eprintln!(
                        "  {} vendor `{}` has no resumable session — hosting a fresh advisor per punt (no fork).",
                        "◆".dimmed(),
                        vendor.as_str()
                    );
                }
                AdvisorPass::ColdBoot
            }
            other => other,
        };

        let (advisor_uuid, forked_from) = match &pass {
            AdvisorPass::Fork(plan) => match advisor::execute_fork(plan) {
                Ok(_) => (plan.fork_uuid.clone(), Some(plan.live.clone())),
                Err(e) => {
                    if !self.json {
                        eprintln!(
                            "  {} advisor fork failed ({e}); falling back to cold-boot.",
                            "◆".yellow()
                        );
                    }
                    (uuid::Uuid::now_v7().to_string(), None)
                }
            },
            AdvisorPass::ColdBoot => (uuid::Uuid::now_v7().to_string(), None),
        };
        let is_fork = forked_from.is_some();

        let log_path = self
            .project_root
            .join(".aida")
            .join("headless-logs")
            .join(format!(
                "advise-{}-{}.jsonl",
                self.spec,
                &advisor_uuid[..advisor_uuid.len().min(8)]
            ));
        if let Some(dir) = log_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let log = std::fs::File::create(&log_path).map_err(|e| {
            PhaseFailure::of(
                FailureKind::Spawn,
                format!("advisor tier: could not create the headless log: {e}"),
            )
        })?;
        if !self.json {
            if let Some(live) = forked_from.as_ref() {
                let mb = live.jsonl_size_bytes as f64 / (1024.0 * 1024.0);
                eprintln!(
                    "  {} forking live advisor session {} ({:.2} MB transcript, via {}) for the design-fork…",
                    "◆".cyan(),
                    &live.uuid[..live.uuid.len().min(8)],
                    mb,
                    live.discovery.label(),
                );
            } else {
                eprintln!(
                    "  {} spawning a headless advisor (cold-boot) for the design-fork…",
                    "◆".cyan()
                );
            }
        }
        let tee_label = if is_fork { "advisor-fork" } else { "advisor" };
        let tee_opts =
            crate::headless_tee::TeeOptions::from_env_and_flag(false).with_label(tee_label);
        let tee_handle = crate::headless_tee::start_tee(&log_path, &tee_opts);
        // STORY-626: the cold-boot branch is context-poor — seed it with the
        // live advisor context file (same prepend the assess cold-boot uses),
        // so unattended punt decisions match the live session. trace:STORY-626
        // TASK-894: build the per-vendor `(program, args)` — Claude resumes the
        // forked session or cold-boots; Codex always hosts a fresh `codex exec`
        // (no resume). `is_fork` is always false for a non-Claude vendor (the
        // pass was forced to cold-boot above), so the fork JSONL is never read.
        // trace:TASK-894 | ai:claude
        // TASK-1045: for a non-Claude vendor the `/aida-advise` slash token is
        // inert (Codex/Gemini don't read `.claude/skills/`), so inline the
        // embedded skill body; Claude keeps the slash form and expands it.
        let is_claude = matches!(vendor, session::HeadlessVendor::Claude);
        let seeded = crate::intake::seeded_advise_prompt_for_vendor(&self.project_root, is_claude);
        let advisor_tuning = session::resolve_agent_tuning(
            &self.project_root,
            vendor,
            aida_core::agents_config::AgentSeat::Advisor,
        );
        // BUG-2133: a contained drain contains its advisor child — the
        // advisor reads untrusted PR content and runs commands. Scoped to the
        // advisor's cwd (the project root, where it writes its punt response).
        let contained_in = self.contained.then_some(self.project_root.as_path());
        let (program, advisor_args) = session::advisor_tier_program_and_args_with_posture(
            vendor,
            is_fork,
            &seeded,
            &advisor_uuid,
            advisor_tuning.model.as_deref(),
            advisor_tuning.effort.as_deref(),
            contained_in,
        );
        let (program, advisor_args) = match contained_child_program_and_args(
            vendor,
            self.contained,
            contained_in,
            "advisor session",
            &program,
            advisor_args,
        ) {
            Ok(pair) => pair,
            Err(e) => {
                tee_handle.stop();
                return Err(PhaseFailure::of(
                    FailureKind::Spawn,
                    format!("advisor tier: {e:#}"),
                ));
            }
        };
        // trace:TASK-1169 | ai:claude
        let (ceiling_key, ceiling_value) = crate::bg_wait_ceiling_env(Some(&self.project_root));
        let mut child_cmd = build_phase_driver_advisor_command(
            &self.project_root,
            &program,
            &advisor_args,
            request_path,
            response_path,
            &ceiling_key,
            &ceiling_value,
            vendor,
            self.contained,
        );
        let status = child_cmd
            .stdout(std::process::Stdio::from(log))
            .status_retrying_etxtbsy()
            .map_err(|e| {
                PhaseFailure::of(
                    FailureKind::Spawn,
                    format!("advisor tier: could not launch the advisor session: {e}"),
                )
            })?;
        tee_handle.stop();

        // Clean up the fork JSONL if the config asks us to. Default is to
        // keep it for audit. Errors are non-fatal.
        if is_fork && !advisor_cfg.keep_fork_jsonls {
            if let AdvisorPass::Fork(plan) = &pass {
                let _ = std::fs::remove_file(&plan.fork_jsonl);
            }
        }
        if !status.success() {
            return Err(PhaseFailure::new(format!(
                "the headless advisor session exited {} — see {}",
                status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "with a signal".to_string()),
                log_path.display(),
            )));
        }
        let response = punt::read_punt_response(response_path).ok_or_else(|| {
            PhaseFailure::of(
                FailureKind::Internal,
                format!(
                    "the headless advisor wrote no usable response — see {}",
                    log_path.display()
                ),
            )
        })?;
        Ok((response, log_path))
    }
}

/// Decide whether phase 2 can safely continue after phase 1 removed its
/// recorded worktree. Both durable views must identify the same published
/// commit; otherwise the push/teardown guard has lost the evidence it needs.
// trace:BUG-1823 | ai:codex
pub(crate) fn missing_implementer_worktree_push_gate(
    worktree: &std::path::Path,
    branch: &str,
    pr: u32,
    remote_head: Option<&str>,
    pr_head: Option<&str>,
) -> Result<(), auto_complete::PhaseFailure> {
    let evidence_failure = |detail: &str| {
        auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::CiUnavailable,
            format!(
                "implementer worktree `{}` disappeared before phase-2 CI/teardown, and {detail}",
                worktree.display()
            ),
        )
    };
    let remote_head = remote_head
        .filter(|head| !head.trim().is_empty())
        .ok_or_else(|| {
            evidence_failure(&format!(
                "origin branch `{branch}` could not be verified as published"
            ))
        })?;
    let pr_head = pr_head
        .filter(|head| !head.trim().is_empty())
        .ok_or_else(|| evidence_failure(&format!("PR/MR {pr} head could not be read")))?;
    if !remote_head.eq_ignore_ascii_case(pr_head) {
        return Err(evidence_failure(&format!(
            "origin branch `{branch}` is at `{remote_head}` while PR/MR {pr} is at `{pr_head}`"
        )));
    }
    Ok(())
}

/// BUG-2133: the fail-closed gate + OS wrapper every agent child a drain
/// spawns directly goes through. Uncontained: `(program, args)` unchanged.
/// Contained: refuse a child that cannot be contained
/// ([`session::require_child_containment`]), then apply the same opt-in
/// `bwrap` OS boundary (`[contained] os_wrap` / `AIDA_OS_WRAP`) the contained
/// implementer gets, rooted at the child's cwd `contained_in` — which itself
/// refuses when os_wrap is requested but bwrap cannot run.
// trace:BUG-2133 | ai:claude
pub(crate) fn contained_child_program_and_args(
    vendor: session::HeadlessVendor,
    contained: bool,
    contained_in: Option<&std::path::Path>,
    child: &str,
    program: &str,
    args: Vec<String>,
) -> anyhow::Result<(String, Vec<String>)> {
    if !contained {
        return Ok((program.to_string(), args));
    }
    session::require_child_containment(vendor, contained, child)?;
    let root = contained_in.ok_or_else(|| {
        anyhow::anyhow!("[contained] no working directory to scope the {child} to — refusing to launch it uncontained")
    })?;
    session::os_wrapped_program_and_args(root, program, args)
}

// trace:BUG-2124 | ai:antigravity
// Builds advisor command for advisor_tier_program_and_args argv.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_phase_driver_advisor_command(
    project_root: &std::path::Path,
    program: &str,
    advisor_args: &[String],
    request_path: &std::path::Path,
    response_path: &std::path::Path,
    ceiling_key: &str,
    ceiling_value: &str,
    vendor: session::HeadlessVendor,
    // trace:BUG-2133 | ai:claude
    contained: bool,
) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    cmd.current_dir(project_root)
        .args(advisor_args)
        .env("AIDA_HEADLESS", "1")
        .env(punt::REQUEST_FILE_ENV, request_path)
        .env(punt::RESPONSE_FILE_ENV, response_path)
        .env("AIDA_SESSION_ROLE", "advisor")
        .env(ceiling_key, ceiling_value);
    if session::containment_applied(vendor, contained) {
        cmd.env("AIDA_CONTAINED", "1");
    } else {
        cmd.env_remove("AIDA_CONTAINED");
    }
    cmd
}

impl RealPhaseDriver {
    pub(crate) fn ensure_implementer_branch_pushed(
        &self,
        branch: &str,
    ) -> Result<(), auto_complete::PhaseFailure> {
        let Some(worktree) = self.implementer_worktree.as_deref() else {
            // BUG-1151 (BUG-1145 residual): the open-PR recovery re-enters at Ci
            // with no local worktree recorded (the lease-match failure returned
            // before it was set). The branch is already pushed — the PR exists
            // remotely — so there is nothing to push here. Only error when this
            // is NOT the recovery path (no PR / not from_pr), where a missing
            // worktree is a genuine internal bug. trace:BUG-1151 | ai:claude
            if self.from_pr || self.pr_number.is_some() {
                return Ok(());
            }
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::Internal,
                "internal: implementer worktree not recorded before the CI phase",
            ));
        };
        if !worktree.is_dir() {
            let pr = self.pr_number.ok_or_else(|| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::Internal,
                    format!(
                        "implementer worktree `{}` disappeared before phase-2 CI/teardown and no verified PR/MR was recorded",
                        worktree.display()
                    ),
                )
            })?;
            let remote_head = dispatched_branch_head_sha(&self.project_root, branch);
            let pr_head = pr_head_sha_best_effort(self, pr);
            return missing_implementer_worktree_push_gate(
                worktree,
                branch,
                pr,
                remote_head.as_deref(),
                pr_head.as_deref(),
            );
        }
        ensure_implementer_branch_pushed(worktree, branch, self.json)
    }

    /// End the implementer session: `aida session end <lease> --yes --skip-ci`.
    /// Releases the lease, returns the warm-pool worktree to idle, and (only
    /// when an OPEN PR still exists on the branch) auto-queues the `Review
    /// PR-N` hand-off. Shared by phase 2 (`finish_ci`) and the BUG-709
    /// AlreadyMerged completion so a self-merged drive tears its session down
    /// the same way a normal drive does, instead of leaking the lease + pool
    /// worktree. A merged PR leaves no open PR, so no spurious reviewer item is
    /// filed.
    // trace:BUG-711 | ai:claude
    pub(crate) fn end_implementer_session(&self) -> Result<(), auto_complete::PhaseFailure> {
        // BUG-1145: the phase-1 recovery path (open PR found after a lease-match
        // failure) re-enters at Ci with NO matched implementer lease recorded —
        // discover_orchestrated_lease failed before self.implementer_lease was
        // set. There is then no session to end here; the orphan lease/worktree
        // is cleaned by a later `aida session reap`. Treat a missing lease as a
        // best-effort no-op rather than an Internal failure that would shelve
        // the (successful) PR. trace:BUG-1145 | ai:claude
        let Some(lease) = self.implementer_lease.clone() else {
            return Ok(());
        };
        let status = std::process::Command::new(self.aida_exe())
            .current_dir(&self.project_root)
            .args(["session", "end", &lease, "--yes", "--skip-ci"])
            .status_retrying_etxtbsy()
            // A spawn failure here is local subprocess plumbing, not red CI —
            // tag it `Spawn` so the hint says so. trace:BUG-218 | ai:claude
            .map_err(|e| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::Spawn,
                    format!("could not run `aida session end`: {e}"),
                )
            })?;
        if !status.success() {
            return Err(auto_complete::PhaseFailure::new(
                "could not end the implementer session — it may have uncommitted \
                 changes; commit or discard them, then re-run",
            ));
        }
        Ok(())
    }

    /// TASK-1230: best-effort removal of a FAILED phase-1 attempt's orphan
    /// worktree during open-PR recovery. NEVER fails/shelves the spec — a
    /// removal problem is logged and swallowed; the drain proceeds to CI on the
    /// verified PR exactly as before. Safety is the pure
    /// [`auto_complete::recovery_worktree_teardown_decision`] gate: a dirty,
    /// locked, or unpushed-local-commit worktree is kept + logged.
    ///
    /// Discovery is deliberately conservative. The recovery path usually has no
    /// recorded lease/worktree (the BUG-1151 reason), so we fall back to
    /// matching the UNIQUE registered worktree checked out on the PR branch —
    /// and we HARD-GUARD against ever removing the main checkout or the
    /// `.aida-store` worktree. Ambiguity (zero or >1 match) → do nothing.
    // trace:TASK-1230 | ai:claude
    pub(crate) fn teardown_failed_attempt_worktree(&mut self, pr_branch: &str) {
        let log = |msg: String| {
            if !self.json {
                eprintln!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    msg
                );
            }
        };

        // The one path we never touch, whatever else happens.
        let main_root = find_main_worktree_root().unwrap_or_else(|_| self.project_root.clone());
        let store_wt = main_root.join(".aida-store");

        // Prefer a worktree the driver already recorded for the failed attempt;
        // otherwise discover the unique one on the PR branch.
        let branch = if !pr_branch.is_empty() {
            pr_branch.to_string()
        } else {
            self.branch.clone().unwrap_or_default()
        };
        let candidate = self
            .implementer_worktree
            .clone()
            .or_else(|| self.retry_implementer_worktree.clone())
            .or_else(|| {
                unique_worktree_on_branch(&self.project_root, &branch, &main_root, &store_wt)
            });

        let Some(worktree) = candidate else {
            log(format!(
                "no unambiguous orphan worktree for {} on `{}` — leaving cleanup to `aida session reap`",
                self.spec, branch
            ));
            return;
        };

        // HARD GUARD: never remove the main checkout or the store worktree, even
        // if a recorded field somehow points at one.
        let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        if canon(&worktree) == canon(&main_root) || canon(&worktree) == canon(&store_wt) {
            log(format!(
                "refusing to tear down {} — it is the main/store worktree",
                worktree.display()
            ));
            return;
        }

        let dirty = aida_core::git_ops::worktree_is_dirty(&worktree);
        let locked = worktree_is_locked(&self.project_root, &worktree);
        let unpushed = local_commits_not_on_branch(&worktree, &branch);
        match auto_complete::recovery_worktree_teardown_decision(dirty, locked, unpushed) {
            auto_complete::RecoveryTeardown::Keep(reason) => {
                log(format!(
                    "keeping the failed attempt's worktree {} — {reason}",
                    worktree.display()
                ));
            }
            auto_complete::RecoveryTeardown::Remove => {
                // Never tear down a tree a live process is still using.
                // trace:BUG-1947 | ai:claude
                if let Err(in_use) = aida_core::worktree_holders::check_worktree_removal(&worktree)
                {
                    log(format!(
                        "keeping the failed attempt's worktree {} — {}",
                        worktree.display(),
                        in_use.summary()
                    ));
                    return;
                }
                match aida_core::git_ops::remove_worktree_at(&self.project_root, &worktree, false) {
                    Ok(()) => {
                        log(format!(
                            "removed the failed attempt's worktree {} (clean, its work is on PR branch `{}`)",
                            worktree.display(),
                            branch
                        ));
                        // Release the failed attempt's lease so `aida ps` /
                        // `aida session leases` stop listing a gone worktree.
                        if let Some(lease) = self.implementer_lease.clone() {
                            let _ = std::process::Command::new(self.aida_exe())
                                .current_dir(&self.project_root)
                                .args(["session", "end", &lease, "--yes", "--skip-ci"])
                                .status_retrying_etxtbsy();
                        }
                    }
                    Err(e) => log(format!(
                        "worktree {} teardown failed ({e}) — harmless; `aida session reap` will retry",
                        worktree.display()
                    )),
                }
            }
        }
    }

    /// STORY-492: seed the branch + PR a `--resume-drain` re-entry skipped
    /// phases would have discovered, so the resumed phases (CI / reviewer /
    /// merge / pull) have the context they need. Called once before
    /// `orchestrate_with_resume` on a resume. trace:STORY-492 | ai:claude
    pub(crate) fn seed_resume_state(
        &mut self,
        branch: Option<String>,
        pr: Option<u32>,
        head_sha: Option<String>,
        ci_terminal_sha: Option<String>,
        ci_terminal_green: Option<bool>,
    ) {
        if branch.is_some() {
            self.branch = branch;
        }
        if pr.is_some() {
            self.pr_number = pr;
        }
        if head_sha.is_some() {
            self.phase_done_head = head_sha;
        }
        if ci_terminal_sha.is_some() {
            self.ci_terminal_sha = ci_terminal_sha;
        }
        if ci_terminal_green.is_some() {
            self.ci_terminal_green = ci_terminal_green;
        }
    }

    /// TASK-136: one phase-1 PR-verify attempt, factored out of
    /// [`Self::run_implementer`] so the transient GH-unreachable case can be
    /// retried with a backoff. Encapsulates the BUG-223 branch-then-spec
    /// fallback and the BUG-257 `git ls-remote` narrowing. Mutates `self.branch`
    /// when the spec-id search recovers a swapped branch. trace:TASK-136
    pub(crate) fn detect_phase1_pr(&mut self, branch: &str) -> Phase1PrResolve {
        // STORY-516: forge-routed branch lookup (view op). The inner spec-id
        // search (detect_open_pr_for_spec) is the LIST op — routed in a later
        // slice — so it stays PrLookup here. The BUG-444/BUG-257 narrowing
        // bodies are unchanged; only the outer variant names move to
        // ChangeLookup. trace:STORY-516 trace:BUG-444 trace:BUG-257 | ai:claude
        // TASK-1421: the lookups go through the driver's forge seam; with no
        // injected factory this is the same `forge_for` dispatch as
        // `change_lookup_for_branch`. trace:TASK-1421 | ai:claude
        let open_for_branch = pr_lookup_from_change_lookup(
            self.project_forge()
                .change_for_branch(branch)
                .unwrap_or_else(|e| crate::forge::ChangeLookup::CliFailed(format!("{e:#}"))),
        );
        match change_lookup_from_pr_lookup_for_branch(open_for_branch, branch) {
            crate::forge::ChangeLookup::Found(c) => Phase1PrResolve::Found(OpenPrInfo {
                number: c.id,
                title: c.title.unwrap_or_default(),
                url: c.url,
                head_branch: (!c.branch.is_empty()).then_some(c.branch),
            }),
            crate::forge::ChangeLookup::NoChange => {
                let open_for_spec = pr_lookup_from_change_lookup(
                    self.project_forge()
                        .change_for_spec(&self.spec)
                        .unwrap_or_else(|e| {
                            crate::forge::ChangeLookup::CliFailed(format!("{e:#}"))
                        }),
                );
                match open_for_spec {
                    PrLookup::Found(pr) => {
                        // Realign the branch so the CI / merge phases probe the
                        // PR's actual head, not the worktree HEAD the branch-keyed
                        // lookup just missed on. trace:BUG-223
                        if let Some(head) = pr_head_branch(&self.project_root, pr.number) {
                            self.branch = Some(head);
                        }
                        eprintln!(
                            "  {} no open PR on branch `{}` — recovered PR-{} via a `{}` \
                         search (the implementer branch was swapped by /aida-pr's \
                         merged-branch guard)",
                            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                            branch,
                            pr.number,
                            self.spec,
                        );
                        Phase1PrResolve::Found(pr)
                    }
                    // BUG-444: both the `--head` and `--search` lookups returned
                    // empty-BUT-SUCCESSFUL. These are eventually-consistent GitHub
                    // LIST queries (`--search` hits the search index, which lags PR
                    // creation by seconds), so an empty result right after
                    // `/aida-pr` ran `gh pr create` does NOT mean "no PR" — it
                    // usually means "not indexed yet". If the branch is on origin
                    // the session pushed, so a PR very likely exists: RETRY within
                    // the eventual-consistency window (reusing the TASK-136 backoff)
                    // instead of declaring a false NoPr. Only a branch that is
                    // genuinely absent from origin is an immediate NoPr (the session
                    // never pushed). This was the dominant drain-failure cause:
                    // ~98% of sessions ship a PR, yet a whole batch could false-fail
                    // by racing the index. trace:BUG-444 EPIC-33 | ai:claude
                    _ => {
                        let origin = probe_branch_on_origin(&self.project_root, branch);
                        if empty_phase1_lookup_is_definitive_nopr(&origin) {
                            Phase1PrResolve::NoPr
                        } else if let PrLookup::Found(merged) = pr_lookup_from_change_lookup(
                            self.project_forge()
                                .merged_change_for_branch(branch)
                                .unwrap_or_else(|e| {
                                    crate::forge::ChangeLookup::CliFailed(format!("{e:#}"))
                                }),
                        ) {
                            // BUG-709: no OPEN PR but the branch IS on origin —
                            // before assuming eventual-consistency lag and
                            // retrying, check whether the branch's PR already
                            // MERGED. A codex (or any) implementer that ran the
                            // full ship itself (create + CI + merge via `aida pr
                            // ship`) leaves a MERGED PR and no open one, so the
                            // open-PR verify would otherwise spin to its retry
                            // ceiling and false-negative a shipped drive as
                            // "inconclusive, retry". A merged PR means the work
                            // landed — resolve AlreadyMerged so the drive
                            // completes cleanly. trace:BUG-709 | ai:claude
                            Phase1PrResolve::AlreadyMerged(merged)
                        } else {
                            let reason = match origin {
                                BranchOriginProbe::Present => format!(
                                    "no open PR indexed yet for `{branch}` (and the \
                                 `{spec}` search), but the branch is on origin — \
                                 likely GitHub eventual-consistency lag after \
                                 `gh pr create`",
                                    spec = self.spec
                                ),
                                _ => format!(
                                    "no open PR found for `{branch}` and `git ls-remote` \
                                 could not confirm whether the branch is on origin — \
                                 retrying before concluding no PR exists"
                                ),
                            };
                            Phase1PrResolve::Retry(reason)
                        }
                    }
                }
            }
            crate::forge::ChangeLookup::CliMissing => {
                Phase1PrResolve::Fail(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::MissingTool,
                    "`gh` is not on PATH — auto-complete needs it to track the PR",
                ))
            }
            crate::forge::ChangeLookup::CliFailed(why) => {
                Phase1PrResolve::Fail(auto_complete::PhaseFailure::new(format!(
                    "could not look up the PR for `{branch}`: {why}"
                )))
            }
            // BUG-257: the GH API was unreachable — *transient*, not "no PR".
            // Narrow with `git ls-remote` (git protocol, separate from the
            // HTTPS API): if the branch isn't on origin no PR can exist, so
            // that collapses to a genuine NoPR failure. Otherwise it is a
            // retryable inconclusive. trace:BUG-257 TASK-136 | ai:claude
            crate::forge::ChangeLookup::Unreachable(why) => {
                match probe_branch_on_origin(&self.project_root, branch) {
                    BranchOriginProbe::Absent => {
                        Phase1PrResolve::Fail(auto_complete::PhaseFailure::of(
                            auto_complete::FailureKind::NoPr,
                            format!(
                                "GH API unreachable ({why}), and `git ls-remote` confirms \
                             branch `{branch}` is not on origin — no PR can exist. \
                             Resume the session and push: `aida queue work {spec} --resume`",
                                spec = self.spec
                            ),
                        ))
                    }
                    BranchOriginProbe::Present => Phase1PrResolve::Retry(format!(
                        "GH API unreachable ({why}); branch `{branch}` is on origin \
                     so a PR may exist — cannot confirm without the API"
                    )),
                    BranchOriginProbe::LsRemoteFailed => Phase1PrResolve::Retry(format!(
                        "GH API unreachable ({why}); `git ls-remote` also failed — \
                     cannot determine whether a PR exists for `{branch}`"
                    )),
                }
            }
        }
    }
}

impl RealPhaseDriver {
    /// Finish adopting the PR found after an implementer resumes. Keep this
    /// transition independently testable: a resumed session may reuse a
    /// draft PR just like phase 1 does.
    // trace:BUG-1791 | ai:codex
    pub(crate) fn adopt_resumed_pr(&mut self, pr: OpenPrInfo) -> auto_complete::ImplementerOutcome {
        self.undraft_reused_publication(&pr);
        self.set_pr_number(pr.number as u32);
        if let Some(head) = pr_head_branch(&self.project_root, pr.number) {
            self.branch = Some(head);
        }
        auto_complete::ImplementerOutcome::PrOpened
    }
}

/// Keep the production retry cadence patient while letting the drain-level
/// `finish_ci` regression exercise all retries without making the unit suite
/// sleep for thirty seconds. The retry count and control flow are identical.
// trace:BUG-1265 | ai:codex
pub(crate) fn ci_red_refine_poll_interval() -> std::time::Duration {
    if cfg!(test) {
        std::time::Duration::from_millis(1)
    } else {
        std::time::Duration::from_secs(15)
    }
}

/// TASK-975: how much of the failing-check log tail rides in the CI-fix
/// prompt. The failure detail lives at the end of a CI log; the cap keeps the
/// prompt bounded on a pathologically chatty run.
// trace:TASK-975 | ai:claude
pub(crate) const CI_FIX_LOG_TAIL_BYTES: usize = 16_000;

/// TASK-975: fetch the failing-step logs for a CI run (`gh run view <id>
/// --log-failed`), tail-bounded. `None` when `gh` is missing, the run cannot
/// be read, or the log is empty — the prompt then tells the fix session to
/// fetch the checks itself.
// trace:TASK-975 | ai:claude
pub(crate) fn failing_ci_log_excerpt(
    project_root: &std::path::Path,
    run_id: &str,
) -> Option<String> {
    let gh = resolve_gh_binary()?;
    let out = std::process::Command::new(gh)
        .current_dir(project_root)
        .args(["run", "view", run_id, "--log-failed"])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(tail_bounded(trimmed, CI_FIX_LOG_TAIL_BYTES))
}

/// TASK-975: keep the LAST `max_bytes` of `text` (on a char boundary) with a
/// truncation marker — a CI log's failure detail is at its tail.
// trace:TASK-975 | ai:claude
pub(crate) fn tail_bounded(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…(log truncated)…\n{}", &text[start..])
}

/// TASK-975: the prompt handed to the headless CI-fix session. Pure so the
/// contract lines (smallest fix, push to the existing branch, never a new PR,
/// exit without pushing when unfixable) are pinned by a unit test.
// trace:TASK-975 | ai:claude
pub(crate) fn build_ci_fix_prompt(
    spec: &str,
    branch: &str,
    failure_reason: &str,
    log_excerpt: &str,
) -> String {
    let log_block = if log_excerpt.is_empty() {
        "No failing-check log could be fetched — run `gh pr checks` / \
         `gh run view --log-failed` yourself to see the failure."
            .to_string()
    } else {
        format!("Failing-check log (tail):\n```\n{log_excerpt}\n```")
    };
    format!(
        "You are a CI-fix session inside an autonomous drain. The PR branch \
         `{branch}` implementing {spec} has failing CI checks.\n\
         \n\
         Reported failure: {failure_reason}\n\
         \n\
         {log_block}\n\
         \n\
         Your job, in this worktree (the branch is already checked out):\n\
         1. Read the spec for context: `aida show {spec} --full`.\n\
         2. Diagnose the failing checks from the log above and make the \
         smallest correct fix. Do not rewrite unrelated code.\n\
         3. Run the relevant local checks (build / fmt / clippy / tests) \
         until they pass.\n\
         4. Commit with a message crediting {spec} and push to the existing \
         branch: `git push origin {branch}`.\n\
         \n\
         Hard rules: do NOT open a new PR, do NOT merge, do NOT force-push, \
         do NOT switch branches. If the failure is not fixable from this \
         worktree (broken infrastructure, a flaky external service), exit \
         WITHOUT committing or pushing anything."
    )
}

/// TASK-975: the current head SHA of `branch` on origin, or `None` when the
/// remote cannot be read. Ground truth for "did the fix session push?".
// trace:TASK-975 | ai:claude
pub(crate) fn remote_head_sha(project_root: &std::path::Path, branch: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["ls-remote", "origin", &format!("refs/heads/{branch}")])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// TASK-84: pure resolver for `aida queue work` permission-mode.
/// Order (first non-None wins):
///   1. `flag`   — `--permission-mode` on the command line
///   2. `env`    — `AIDA_PERMISSION_MODE` env var
///   3. `config` — `.aida/config.toml [behavior] permission_mode`
///   4. `is_aida_worktree=true` default → `bypassPermissions` (worktree is
///      git-sandboxed; prompt flood ate autonomous runs — see TASK-84
///      description)
///   5. fallback → `acceptEdits` (safe default for non-AIDA cwd)
///      Returns the mode + a short origin string for the pre-flight summary.
///      Pure decision helper — kept separate so unit tests can pin the
///      resolution order without env/config side effects. trace:TASK-84 | ai:claude
///      STORY-495: resolve the effective `--permission-mode` for an interactive
///      `aida queue work` launch. `None` → honor Claude's native posture (no flag
///      injected — the faithful default). Precedence, highest first:
///   1. `--permission-mode` flag (explicit override)
///   2. `AIDA_PERMISSION_MODE` env (process-level opt-in)
///   3. `.aida/config.toml [behavior] permission_mode` (project policy)
///   4. `[agents] bypass` knob → `bypassPermissions` (uniform fleet opt-in)
///   5. otherwise → `None` (native)
///
/// Pre-STORY-495 this defaulted to `bypassPermissions` inside an AIDA worktree
/// and `acceptEdits` elsewhere; both auto-injections are gone — the uniform
/// knob is the single place that restores bypass posture. trace:STORY-495
pub(crate) fn resolve_queue_work_permission_mode(
    flag: Option<&str>,
    env: Option<&str>,
    config: Option<&str>,
    bypass_knob: bool,
    plan_only: bool,
) -> (Option<String>, &'static str) {
    if let Some(m) = flag.filter(|s| !s.is_empty()) {
        return (Some(m.to_string()), "--permission-mode flag");
    }
    // STORY-265: a plan-only session is read-only by default — it writes a
    // docs/plans/ file, not code. An explicit --permission-mode flag (above)
    // still wins; otherwise `plan` overrides env/config/bypass so the planning
    // session can't edit code even on a bypass-by-default project.
    if plan_only {
        return (Some("plan".to_string()), "--plan-only (read-only default)");
    }
    if let Some(m) = env.filter(|s| !s.is_empty()) {
        return (Some(m.to_string()), "AIDA_PERMISSION_MODE env");
    }
    if let Some(m) = config.filter(|s| !s.is_empty()) {
        return (Some(m.to_string()), ".aida/config.toml");
    }
    if bypass_knob {
        return (
            Some("bypassPermissions".to_string()),
            "[agents] bypass knob",
        );
    }
    (None, "native (faithful default)")
}

/// STORY-287: the three-mode autonomy ladder for `aida queue work`. Aligns
/// the human's role with the implementer's pause behavior:
///   - `Default`  — human is driving; every prompt pauses.
///   - `Zen`      — advisor on standby; mechanical `kind:confirmation`
///     prompts auto-resolve, `kind:design-fork` prompts pause.
///   - `NoHuman`  — nobody reachable; the headless drain (STORY-263).
///     trace:STORY-287 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutonomyMode {
    Default,
    Zen,
    NoHuman,
}

impl AutonomyMode {
    /// ADR-7/ADR-10: the SINGLE in-process resolution of an in-flight
    /// `--auto-complete` run's autonomy mode. Unifies both autonomy axes into
    /// one typed value — the typed `--no-human` mode (which gates PARENT-engine
    /// behavior: headless implementer spawn, CI-watch) plus `--zen` (a
    /// CHILD-session signal: skill templates auto-resolve `kind:confirmation`
    /// prompts when `AIDA_ZEN` is set).
    ///
    /// `--zen` is recovered from the zen-INTENT TOKEN (`AIDA_ZEN_TOKEN`), not a
    /// bare `AIDA_ZEN` read: the token is minted only by the `--zen` dispatch
    /// arm and scrubbed by the `Default` / `--no-human` arms, so a leaked
    /// `AIDA_ZEN=1` is NOT mistaken for a zen run (BUG-237). The `AIDA_ZEN` env
    /// var remains the cross-process TRANSPORT to spawned phase children / skill
    /// templates; this typed value is the in-process SOURCE OF TRUTH, so the
    /// engine path no longer re-derives zen-ness from scattered bare env reads.
    /// ADR-10 carries this resolved value onto the phase driver as the
    /// `autonomy_mode` field (symmetric with `no_human`); in-process zen
    /// branches read `RealPhaseDriver::is_zen_run()`, not a bare env re-read.
    // trace:ADR-7 trace:ADR-10 | ai:claude
    pub(crate) fn for_auto_complete_run(no_human: Option<auto_complete::NoHumanMode>) -> Self {
        Self::resolve_run(no_human, std::env::var(zen::ZEN_TOKEN_ENV).is_ok())
    }

    /// The pure core of [`for_auto_complete_run`]: resolve the autonomy mode
    /// from the typed `--no-human` mode and whether the zen-intent token is
    /// present. Precedence `--no-human` > `--zen` > default matches
    /// [`resolve_autonomy_mode`]. Pure, so the precedence + the BUG-237
    /// leak-resistance are unit-testable without mutating process env.
    // trace:ADR-7 trace:ADR-10 | ai:claude
    pub(crate) fn resolve_run(
        no_human: Option<auto_complete::NoHumanMode>,
        zen_token_present: bool,
    ) -> Self {
        match (no_human.is_some(), zen_token_present) {
            (true, _) => AutonomyMode::NoHuman,
            (false, true) => AutonomyMode::Zen,
            (false, false) => AutonomyMode::Default,
        }
    }

    /// True when this run is a supervised `--zen` drive.
    // trace:ADR-10
    pub(crate) fn is_zen(self) -> bool {
        matches!(self, AutonomyMode::Zen)
    }
}

/// Resolve the effective autonomy mode from the `--zen` flag and the
/// presence of `--no-human`. Precedence is `--no-human` > `--zen` >
/// default — when both `--zen` and `--no-human` are set, `--no-human`
/// wins because it is the strictly stronger mode (it removes the human
/// entirely). Pure, so the precedence rule is unit-testable without
/// spawning a session. trace:STORY-287 | ai:claude
pub(crate) fn resolve_autonomy_mode(zen_flag: bool, no_human: bool) -> AutonomyMode {
    match (no_human, zen_flag) {
        (true, _) => AutonomyMode::NoHuman,
        (false, true) => AutonomyMode::Zen,
        (false, false) => AutonomyMode::Default,
    }
}
