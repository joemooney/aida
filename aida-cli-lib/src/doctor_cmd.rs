//! `aida doctor` command cluster (EPIC-19) — the heavy diagnostics + migration
//! command. `handle_doctor_command` dispatches the read-only multi-agent drift
//! report (`doctor_multi_agent`), the `--heal` remediation path
//! (`heal_doctor_*`), the report renderers, and the migration/repair
//! subcommands (`doctor_fsck`, `doctor_verify_relationships`,
//! `doctor_validate_trace_comments`, `doctor_repair_stale_blocks`,
//! `doctor_scrub_collisions`, `doctor_migrate_counter_scope`, `doctor_fix_sandbox`).
//!
//! Extracted verbatim from `main.rs` (SPIKE-78; pure movement, no behavior
//! change). The shared diagnostics-collection machinery — `collect_doctor_findings`,
//! `scan_completed_without_commit`, `referenced_spec_ids_on_default_branch`,
//! `which_binary`, `normalize_doctor_category`, the worktree-GC classifiers, and
//! the utility helpers (`bwrap_status_line`, `git_config_value`,
//! `humanize_duration_secs`, `salvage_worktree_patch`, `session_gc`, …) — stays
//! in `main.rs` because `aida status --full`/`--ci` (STORY-707) drive it too;
//! this module reaches those via `crate::`.

#![allow(clippy::too_many_arguments)]

use anyhow::Result;
use colored::Colorize;

use crate::*;

pub(crate) fn handle_doctor_command(
    heal: bool,
    yes: bool,
    category: Option<&str>,
    json: bool,
    force: bool,
    all: bool,
    since: Option<&str>,
    cmd: Option<&cli::DoctorCommand>,
) -> Result<()> {
    let Some(cmd) = cmd else {
        return doctor_multi_agent(DoctorRunOptions {
            heal,
            yes,
            category: category.map(str::to_string),
            json,
            force,
            all,
            since: since.map(str::to_string),
            fail_on_findings: false,
        });
    };
    match cmd {
        cli::DoctorCommand::Check {
            category,
            all: sub_all,
            json,
            fail_on_findings,
        } => {
            // TASK-1473: `doctor check runaway-seats` is what the
            // maintenance scheduler dispatches every 15 minutes (the
            // watchdog job) — and the watchdog rule never touches the AIDA
            // store. The full path below always pays for loading the whole
            // store plus `collect_doctor_findings` (parent-tag-drift,
            // id-collisions, lease listing, live-session probing, …) even
            // though `--category runaway-seats` filters every one of those
            // findings back out. Take a lighter entry path straight to the
            // watchdog scan for exactly this shape (single category, no
            // `--all`); anything else falls through to the full path
            // unchanged. trace:TASK-1473 | ai:claude
            if !all
                && !*sub_all
                && normalize_doctor_category(category).ok().as_deref()
                    == Some(crate::runaway_seats::CATEGORY)
            {
                return doctor_check_runaway_seats_light(*json, *fail_on_findings);
            }
            doctor_multi_agent(DoctorRunOptions {
                heal: false,
                yes,
                category: Some(category.clone()),
                json: *json,
                force,
                all: all || *sub_all,
                since: since.map(str::to_string),
                fail_on_findings: *fail_on_findings,
            })
        }
        cli::DoctorCommand::Heal {
            category,
            yes,
            force,
            all: sub_all,
            json,
        } => doctor_multi_agent(DoctorRunOptions {
            heal: true,
            yes: *yes,
            category: Some(category.clone()),
            json: *json,
            force: *force,
            all: all || *sub_all,
            since: since.map(str::to_string),
            fail_on_findings: false,
        }),
        cli::DoctorCommand::MigrateCounterScope {
            to,
            dry_run,
            yes,
            size,
        } => doctor_migrate_counter_scope(to, *dry_run, *yes, *size),
        cli::DoctorCommand::RepairStaleBlocks { dry_run, yes } => {
            doctor_repair_stale_blocks(*dry_run, *yes)
        }
        cli::DoctorCommand::ScrubCollisions => doctor_scrub_collisions(),
        cli::DoctorCommand::VerifyRelationships { repair, yes } => {
            doctor_verify_relationships(*repair, *yes)
        }
        cli::DoctorCommand::ValidateTraceComments {
            strip_dangling,
            dry_run,
            yes,
        } => doctor_validate_trace_comments(*strip_dangling, *dry_run, *yes),
        cli::DoctorCommand::Fsck => doctor_fsck(),
        cli::DoctorCommand::ConventionCheck { quiet } => doctor_convention_check(*quiet),
        cli::DoctorCommand::ShellSubstitutionHoles { exclude } => {
            doctor_shell_substitution_holes(exclude)
        }
    }
}

// The rules intentionally match only anomalies that cannot be explained by a
// wrapped prose line. A broad "two spaces" rule produces table/code noise and
// cannot distinguish a typo from lost command-substitution text.
// trace:TASK-190 | ai:codex
fn shell_substitution_hole_kind(line: &str) -> Option<&'static str> {
    let bytes = line.as_bytes();
    if bytes
        .windows(2)
        .any(|pair| matches!(pair, b"(," | b"[," | b"{,"))
    {
        return Some("opening bracket followed by comma");
    }
    for (index, window) in bytes.windows(3).enumerate() {
        if window == b"'s " {
            let rest = &bytes[index + 3..];
            let spaces = rest.iter().take_while(|byte| **byte == b' ').count();
            if spaces >= 1 && rest.get(spaces).is_some_and(u8::is_ascii_lowercase) {
                return Some("possessive followed by same-line gap");
            }
        }
    }
    None
}

fn doctor_shell_substitution_holes(exclude: &[String]) -> Result<()> {
    let project_root = find_project_root()?;
    let objects_root = shell_substitution_objects_root(&project_root);
    if !objects_root.exists() {
        println!("(no objects/ tree — nothing to check)");
        return Ok(());
    }
    let reqs = aida_core::object_store::load_all_objects(&objects_root)?;
    let mut count = 0usize;
    for req in reqs {
        let id = req.spec_id.as_deref().unwrap_or("<unknown>");
        if exclude.iter().any(|excluded| excluded == id) {
            continue;
        }
        let fields = std::iter::once(("description", req.description.as_str())).chain(
            req.comments
                .iter()
                .map(|comment| ("comment", comment.content.as_str())),
        );
        for (field, body) in fields {
            for (line_number, line) in body.lines().enumerate() {
                if let Some(kind) = shell_substitution_hole_kind(line) {
                    count += 1;
                    println!("{id}:{field}:{}: {kind}: {}", line_number + 1, line.trim());
                }
            }
        }
    }
    println!("{count} candidate(s); inspect before repairing (this check never writes)");
    Ok(())
}

fn shell_substitution_objects_root(project_root: &std::path::Path) -> std::path::PathBuf {
    detect_distributed_store_from(project_root)
        .unwrap_or_else(|| project_root.join(".aida-store"))
        .join("objects")
}

#[cfg(test)]
mod task_190_tests {
    use super::{shell_substitution_hole_kind, shell_substitution_objects_root};
    use std::process::Command;

    #[test]
    fn known_shell_substitution_holes_are_detected() {
        assert!(shell_substitution_hole_kind("trigger (, RealPhaseDriver)").is_some());
        assert!(shell_substitution_hole_kind("creator's  through forge").is_some());
    }

    #[test]
    fn wrapping_tables_and_ordinary_spacing_are_not_candidates() {
        assert_eq!(shell_substitution_hole_kind("the  next step"), None);
        assert_eq!(shell_substitution_hole_kind("name    value"), None);
        assert_eq!(
            shell_substitution_hole_kind("owner's\n  implementation"),
            None
        );
        assert_eq!(shell_substitution_hole_kind("call `date` safely"), None);
    }

    #[test]
    fn detector_resolves_canonical_store_from_sibling_worktree() {
        fn git(cwd: &std::path::Path, args: &[&str]) {
            let output = Command::new("git")
                .current_dir(cwd)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("main");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "--initial-branch=main", "--quiet"]);
        git(&main, &["config", "user.email", "test@example.com"]);
        git(&main, &["config", "user.name", "Test"]);
        std::fs::create_dir_all(main.join(".aida")).unwrap();
        std::fs::write(
            main.join(".aida/config.toml"),
            "[deployment]\nstore_path = \".aida-store\"\n",
        )
        .unwrap();
        git(&main, &["add", ".aida/config.toml"]);
        git(&main, &["commit", "-m", "init", "--quiet"]);
        std::fs::create_dir_all(main.join(".aida-store/objects")).unwrap();

        let sibling = temp.path().join("sibling");
        git(
            &main,
            &[
                "worktree",
                "add",
                "--quiet",
                sibling.to_str().unwrap(),
                "-b",
                "feature",
            ],
        );
        assert_eq!(
            shell_substitution_objects_root(&sibling)
                .canonicalize()
                .unwrap(),
            main.join(".aida-store/objects").canonicalize().unwrap()
        );
        git(
            &main,
            &["worktree", "remove", "--force", sibling.to_str().unwrap()],
        );
    }
}

/// Ergonomic `aida worktree gc` front door for the existing squash-aware
/// merged-agent-worktree healer. Keep this as a thin wrapper so every caller
/// uses the same scan, destructive-heal gate, and dirty-worktree refusal.
// trace:TASK-1145 | ai:codex
pub(crate) fn run_merged_agent_worktree_gc(yes: bool, force: bool, json: bool) -> Result<()> {
    doctor_multi_agent(DoctorRunOptions {
        heal: true,
        yes,
        category: Some("merged-agent-worktrees".to_string()),
        json,
        force,
        all: false,
        since: None,
        fail_on_findings: false,
    })
}

// ----------------------------------------------------------------------------
// STORY-462 — `aida doctor`: multi-agent state drift diagnostics + healing.
// ----------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct DoctorRunOptions {
    heal: bool,
    yes: bool,
    category: Option<String>,
    json: bool,
    force: bool,
    all: bool,
    /// TASK-673: cutoff for the completed-without-commit integrity check —
    /// specs completed before this ref/date are exempt (legacy history).
    since: Option<String>,
    /// Exit non-zero when the selected category has any finding, so a scheduled
    /// job can GATE on it rather than only report. Opt-in: `doctor check` stays
    /// report-only unless the caller asks, which keeps this a caller's choice
    /// rather than a contract change to a shared surface.
    // trace:STORY-1422 | ai:claude
    fail_on_findings: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub(crate) struct DoctorHealResult {
    category: String,
    id: String,
    action: String,
    status: String,
    detail: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
struct DoctorReport {
    total: usize,
    findings: Vec<DoctorFinding>,
    #[serde(skip_serializing_if = "is_zero_usize")]
    hidden_completed_without_commit: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    healed: Vec<DoctorHealResult>,
    /// TASK-865: read-only bubblewrap OS-sandbox availability status line.
    #[serde(skip_serializing_if = "Option::is_none")]
    bwrap: Option<String>,
    /// Machine boundary consumed by the performance scheduler. This is an
    /// allow-list, not captured stdout.
    // trace:BUG-1573 | ai:codex
    #[serde(skip_serializing_if = "Vec::is_empty")]
    performance_audits: Vec<schedule_ledger::PerformanceAudit>,
    /// STORY-1462: what the runaway-seat watchdog could and could not see.
    // trace:STORY-1462 | ai:claude
    #[serde(skip_serializing_if = "Option::is_none")]
    runaway_seats: Option<crate::runaway_seats::Coverage>,
}

impl DoctorReport {
    fn from_findings(findings: Vec<DoctorFinding>) -> Self {
        Self {
            total: findings.len(),
            findings,
            hidden_completed_without_commit: 0,
            healed: Vec::new(),
            bwrap: Some(bwrap_status_line()),
            performance_audits: Vec::new(),
            runaway_seats: None,
        }
    }
}

fn is_zero_usize(n: &usize) -> bool {
    *n == 0
}

fn doctor_multi_agent(opts: DoctorRunOptions) -> Result<()> {
    let project_root = main_worktree_root_from(&find_project_root()?);
    // A bad `--since` fails the run with an error naming the flag rather than
    // silently applying no cutoff. trace:BUG-1622 | ai:claude
    if let Some(raw) = opts.since.as_deref() {
        resolve_completed_since_cutoff(&project_root, raw)?;
    }
    let store_path = project_root.join(".aida-store");
    let store = Storage::new(&store_path)
        .load()
        .with_context(|| format!("loading AIDA store at {}", store_path.display()))?;
    let mut findings = collect_doctor_findings(&project_root, &store, opts.category.as_deref())?;
    let mut hidden_completed_without_commit = 0;
    let mut performance_audits = Vec::new();

    // TASK-673: the completed-without-commit integrity check runs git scans
    // (a default-branch `git log` + `git grep`) so it is kept OUT of the hot
    // `collect_doctor_findings` path (which `aida status` calls on every
    // invocation) and appended here, where only `aida doctor` reaches it.
    // It honours the same `--category` filter as the built-in categories.
    // trace:TASK-673 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "completed-without-commit")? {
        let env_since = std::env::var("AIDA_DOCTOR_COMPLETED_SINCE")
            .ok()
            .filter(|s| !s.trim().is_empty());
        if let (None, Some(raw)) = (opts.since.as_deref(), env_since.as_deref()) {
            // trace:BUG-1622 | ai:claude
            resolve_completed_since_cutoff(&project_root, raw)
                .context("AIDA_DOCTOR_COMPLETED_SINCE is set to an invalid value")?;
        }
        let since = opts
            .since
            .clone()
            .or(env_since)
            .or_else(default_completed_without_commit_recent_cutoff)
            .filter(|s| !s.trim().is_empty());
        let scan = scan_completed_without_commit_with_options(
            &project_root,
            &store,
            since.as_deref(),
            opts.all,
        );
        hidden_completed_without_commit = scan.hidden_older;
        findings.extend(scan.findings);
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-717: stale-remote-branch scan. Like completed-without-commit it runs
    // git ancestry probes + a `gh pr list`, so it is kept OUT of the hot
    // `collect_doctor_findings` path and appended here where only `aida doctor`
    // reaches it. Honours the same `--category` filter. trace:TASK-717
    if doctor_category_selected(opts.category.as_deref(), "stale-remote-branches")? {
        findings.extend(scan_stale_remote_branches(&project_root, &store));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-878: merged Agent-tool worktree GC. Runs git ancestry probes + a forge
    // merged-PR lookup, so it is kept OUT of the hot `collect_doctor_findings`
    // path and appended here where only `aida doctor` reaches it. Honours the
    // same `--category` filter. trace:TASK-878 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "merged-agent-worktrees")? {
        findings.extend(scan_merged_agent_worktrees(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // STORY-781: project-manifest check. Reads one small file (plus one
    // `git remote get-url` when a repository is recorded), so it is cheap, but
    // it is appended here with the other opt-in categories rather than added
    // to the hot `collect_doctor_findings` path. Honours `--category`.
    // trace:STORY-781 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "project-manifest")? {
        findings.extend(scan_project_manifest(&project_root, &store));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-1095: remote-drift scan — do the shared branches (trunk + store) hold
    // the same tip on every configured remote? Uses cheap `ls-remote` (refs
    // only, no object transfer), so it is kept OUT of the hot
    // `collect_doctor_findings` path and appended here. Honours the same
    // `--category` filter. trace:TASK-1095 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "remote-drift")? {
        findings.extend(scan_remote_drift(&project_root));
        findings.extend(scan_store_mirror_fanout_failures(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // STORY-1127: read-only permission posture check. Kept in the opt-in doctor
    // append path because it inspects user-global and native Codex config files
    // that `aida status` should not need to read on every run.
    // trace:STORY-1127 | ai:codex
    if doctor_category_selected(opts.category.as_deref(), "permission-posture")? {
        findings.extend(crate::config_cmd::scan_permission_posture_findings(
            &project_root,
        ));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // trace:STORY-1043 | ai:codex
    if doctor_category_selected(opts.category.as_deref(), "ci")? {
        if let Some(red) = nightly_red_status(&project_root) {
            findings.push(DoctorFinding {
                category: "ci".to_string(),
                id: "cross-platform-nightly-red".to_string(),
                summary: red.summary,
                action: red
                    .run_id
                    .map(|id| format!("inspect `gh run view {id}` and fix-forward until green"))
                    .unwrap_or_else(|| {
                        "inspect `gh run list --workflow cross-platform.yml`".to_string()
                    }),
                safe_heal: false,
            });
            findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
        }
    }

    // TASK-1124: rule-delivery-rot — hash the DEPLOYED vendor prompts/skills
    // (project `.claude/`+`.codex/` and the machine-global `~/.codex/prompts`)
    // against the binary's embedded source templates and flag drift. Turns the
    // invisible "stale scaffolding at the seams" failure class (a contributing
    // cause of the TASK-1123 reviewer-bypass incident) into a checkable one.
    // Detection only — the fix is the re-scaffold command, not a heal.
    // trace:TASK-1124 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "scaffold-drift")? {
        findings.extend(scan_scaffold_drift(&project_root, &store));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-1122: store-scrub — when identity redaction is configured, has the
    // raw system identity already leaked into the store (a leak that landed
    // before redaction was turned on)? Reads store files + commit authors.
    // trace:TASK-1122 | ai:claude
    // BUG-1505: non-canonical review-verdict files. Report-only — the verdict
    // store is gitignored, so a bulk rewrite would have no history to undo it.
    // trace:BUG-1505 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "review-verdicts")? {
        findings.extend(scan_review_verdicts(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    if doctor_category_selected(opts.category.as_deref(), "store-scrub")? {
        findings.extend(scan_store_scrub(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-1313: round-trip artifacts — a swallowed TOON row header or a
    // UTF-8-as-Latin-1 mojibake sequence left in stored spec text by a
    // read-modify-write through rendered `aida show` output. Pure text scan
    // over the already-loaded store (no extra git/process calls), but kept
    // off the hot `collect_doctor_findings` path per the same convention as
    // the other opt-in categories above: `aida status` should not pay for a
    // full-store text scan on every invocation. Report-only. Honours
    // `--category`. trace:TASK-1313 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "round-trip-artifacts")? {
        findings.extend(scan_round_trip_artifacts(&store));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // The performance gate: guarded command shapes that exceed their budget on
    // TOO LARGE A FRACTION of recent calls, plus guarded shapes with no recorded
    // calls at all. A proportion over a window, not a median — the distribution
    // this judges is bimodal, and a median sits at ~665ms and never trips even
    // with the regression live. Opt-in by config: a project with no
    // `[performance.budgets]` has nothing guarded and nothing to report.
    // trace:STORY-1422 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "performance")? {
        let cfg = crate::read_project_config_value(&project_root);
        let budgets = performance_budgets(cfg.as_ref());
        let policy = performance_policy(cfg.as_ref());
        if !budgets.is_empty() {
            let events = crate::usage::read_events();
            let now = chrono::Utc::now();
            // BUG-1572: `~/.aida/usage.jsonl` is machine-global, so scope the
            // population to binaries in THIS HEAD's lineage before judging.
            // trace:BUG-1572 | ai:claude
            let lineage = resolve_binary_lineage(&project_root, &events, &budgets, &policy, now);
            findings.extend(performance_findings(
                &events, &budgets, &policy, now, &lineage,
            ));
            performance_audits =
                performance_audit_records(&events, &budgets, &policy, now, &lineage);
            findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
        }
    }

    // STORY-1367: free disk headroom on the filesystem holding the project
    // root, checked against a configurable floor. Silent when healthy — a
    // job that reports every run trains people to ignore it, which is how
    // the 2026-09-19 hub-drift warnings were ignored in the first place.
    // Kept in this opt-in append path (not the hot `collect_doctor_findings`
    // path) for the same reason as `remote-drift`: cheap, but not free, and
    // `aida status` should not pay for a disk probe on every invocation.
    // trace:STORY-1367 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "disk-headroom")? {
        let cfg = crate::read_project_config_value(&project_root);
        let min_free_gib = disk_headroom_min_free_gib(cfg.as_ref());
        findings.extend(scan_disk_headroom(&project_root, min_free_gib));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // STORY-1462: the runaway-seat watchdog. Reads the trailing day of this
    // project's session transcripts (incrementally, via a per-file watermark)
    // and trips on per-session wake/token/repeated-prompt/idle anomalies and
    // the project's daily token spend. Zero-token and bounded; opt-in path
    // for the same reason as disk-headroom. The coverage block travels with
    // the report so unavailable evidence reads as unknown, never as ok.
    // trace:STORY-1462 | ai:claude
    let mut runaway_seats = None;
    if doctor_category_selected(opts.category.as_deref(), "runaway-seats")? {
        let cfg = crate::read_project_config_value(&project_root);
        let policy = crate::runaway_seats::policy(cfg.as_ref());
        let label = project_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| project_root.display().to_string());
        let (seat_findings, coverage) = crate::runaway_seats::scan(
            &label,
            &crate::runaway_seats::default_sources(&project_root),
            &policy,
            chrono::Utc::now(),
            &crate::runaway_seats::registry_attribution(&project_root),
        );
        findings.extend(seat_findings);
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
        runaway_seats = Some(coverage);
    }

    let mut report = DoctorReport::from_findings(findings);
    report.performance_audits = performance_audits;
    report.runaway_seats = runaway_seats;
    report.hidden_completed_without_commit = hidden_completed_without_commit;

    if opts.heal {
        report.healed = heal_doctor_findings(&project_root, &report.findings, &opts)?;
    }

    if opts.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render_doctor_report(&report, opts.heal)?;
        if let Some(coverage) = &report.runaway_seats {
            print!("{}", crate::runaway_seats::render_coverage(coverage));
        }
        // STORY-707: `aida doctor` is the check-everything home. The heavy
        // orientation diagnostics that used to ride bare `aida status` — PR/CI
        // (a `gh` network call), live-session/lease liveness, worktree probes,
        // the full fleet roster, and cross-clone coordination — now surface
        // HERE, not on the fast default `aida status`. Gated to the full,
        // unfiltered text run (no `--category` narrow filter) so a targeted
        // `aida doctor check --category X` stays focused. trace:STORY-707
        if opts.category.is_none() {
            print_doctor_status_diagnostics(&project_root, &store);
        }
    }

    // STORY-1127: permission posture is intended as a check/gate category: a
    // flagged full-access or incoherent sandbox state must produce a non-zero
    // exit so headless drains and scripts cannot miss it.
    // trace:STORY-1127 | ai:codex
    if !opts.heal
        && !report.findings.is_empty()
        && opts.category.as_deref().is_some()
        && doctor_category_selected(opts.category.as_deref(), "permission-posture")?
    {
        anyhow::bail!("permission-posture finding(s) detected — see the report above");
    }

    // The opt-in gate. Deliberately GENERAL rather than a second hardcoded
    // category beside the permission-posture check above. That one is the
    // precedent, and adding a second special case is exactly how two categories
    // end up behaving differently under one verb — which a later reader
    // "fixes" in the wrong direction, and the wrong direction here is silence.
    // A caller that wants to be gated asks, and asks in the job definition
    // where the next reader can see that the job is gating.
    // trace:STORY-1422 | ai:claude
    if !opts.heal && opts.fail_on_findings && !report.findings.is_empty() {
        anyhow::bail!(
            "{} finding(s) in {} — failing because --fail-on-findings was requested",
            report.findings.len(),
            opts.category.as_deref().unwrap_or("all categories")
        );
    }

    // BUG-471: heal now continues past a single finding's failure (no more
    // first-error abort), so the failure signal moves to the exit code — bail
    // non-zero after the report so scripts/automation still notice.
    // trace:BUG-471 | ai:claude
    let failed = report
        .healed
        .iter()
        .filter(|r| r.status == "failed")
        .count();
    if failed > 0 {
        anyhow::bail!("{failed} finding(s) failed to heal — see the report above");
    }
    Ok(())
}

/// TASK-1473: the light entry path for `aida doctor check runaway-seats`.
/// Reads `[watchdog]` config off the project root and runs the watchdog scan
/// directly — no `Storage::load()` of the whole AIDA store, no
/// `collect_doctor_findings` pass (parent-tag-drift, id-collisions, lease
/// listing, live-session probing, …), no other opt-in doctor category. The
/// watchdog itself reads only on-disk transcripts/logs (see
/// `runaway_seats.rs` module docs), so none of that machinery was ever
/// needed for this one category; skipping it is what turns the scheduled
/// tick's ~8s `aida doctor` startup into a sub-second run. Output shape
/// (report fields, exit-code gating on `--fail-on-findings`) matches the
/// full path exactly, so a script or the scheduler cannot tell which path
/// ran.
// trace:TASK-1473 | ai:claude
fn doctor_check_runaway_seats_light(json: bool, fail_on_findings: bool) -> Result<()> {
    let project_root = main_worktree_root_from(&find_project_root()?);
    let cfg = crate::read_project_config_value(&project_root);
    let policy = crate::runaway_seats::policy(cfg.as_ref());
    let label = project_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| project_root.display().to_string());
    let (findings, coverage) = crate::runaway_seats::scan(
        &label,
        &crate::runaway_seats::default_sources(&project_root),
        &policy,
        chrono::Utc::now(),
        &crate::runaway_seats::registry_attribution(&project_root),
    );

    let mut report = DoctorReport::from_findings(findings);
    report.runaway_seats = Some(coverage);

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render_doctor_report(&report, false)?;
        if let Some(coverage) = &report.runaway_seats {
            print!("{}", crate::runaway_seats::render_coverage(coverage));
        }
    }

    // Mirrors the `--fail-on-findings` gate in `doctor_multi_agent`.
    if fail_on_findings && !report.findings.is_empty() {
        anyhow::bail!(
            "{} finding(s) in runaway-seats — failing because --fail-on-findings was requested",
            report.findings.len()
        );
    }
    Ok(())
}

/// TASK-1124: which deployed Codex prompts in `dir` (normally `~/.codex/prompts`)
/// have drifted from the current source templates. A prompt that EXISTS but
/// whose content differs from `expected_codex_prompts()` is rot (a stale
/// delivery — e.g. a fix that never re-scaffolded); a MISSING prompt is an
/// opt-out, not rot, so it is never flagged. Pure over `dir`, so unit-testable
/// without touching $HOME.
// trace:TASK-1124 | ai:claude
fn codex_prompts_drift(dir: &std::path::Path) -> Vec<String> {
    let mut drifted = Vec::new();
    for (name, expected) in aida_core::scaffolding::codex_prompts::expected_codex_prompts() {
        let path = dir.join(format!("{name}.md"));
        if let Ok(actual) = std::fs::read_to_string(&path) {
            if !aida_core::scaffolding::generated_text_matches(&actual, &expected) {
                drifted.push(name);
            }
        }
    }
    drifted.sort();
    drifted
}

fn parse_codex_semver(raw: &str) -> Option<(u64, u64, u64)> {
    raw.split(|c: char| !c.is_ascii_digit() && c != '.')
        .find_map(|token| {
            let mut parts = token.split('.');
            let major = parts.next()?.parse().ok()?;
            let minor = parts.next()?.parse().ok()?;
            let patch = parts.next()?.parse().ok()?;
            Some((major, minor, patch))
        })
}

pub(crate) fn installed_codex_version() -> Option<(u64, u64, u64)> {
    let out = std::process::Command::new("codex")
        .arg("--version")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let banner = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    parse_codex_semver(&banner)
}

fn codex_prompt_dir_has_aida_prompt_cruft(dir: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(|entry| entry.ok()).any(|entry| {
        let path = entry.path();
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| {
                name.starts_with("aida-") && (name.ends_with(".md") || name.ends_with(".aida-bak"))
            })
    })
}

fn codex_ignores_prompt_dir_finding_for_version(
    dir: &std::path::Path,
    version: (u64, u64, u64),
) -> Option<DoctorFinding> {
    if !codex_prompt_dir_has_aida_prompt_cruft(dir) {
        return None;
    }
    if !aida_core::scaffolding::codex_prompts::codex_prompt_dir_is_undiscoverable(version) {
        return None;
    }
    Some(DoctorFinding {
        category: "scaffold-drift".to_string(),
        id: "scaffold-drift/codex-prompts-undiscovered".to_string(),
        summary: format!(
            "~/.codex/prompts contains AIDA prompt files, but installed Codex {}.{}.{} does not discover them as `/aida-*` slash commands",
            version.0, version.1, version.2
        ),
        action: "Prune the dead prompt pack (`rm -rf ~/.codex/prompts` or delete its `aida-*.md` and `*.aida-bak` files); use scaffolded `.codex/skills/` via `/skills` or `$aida-*`, or run the matching `aida ...` CLI verb directly".to_string(),
        safe_heal: false,
    })
}

fn codex_ignores_prompt_dir_finding(dir: &std::path::Path) -> Option<DoctorFinding> {
    codex_ignores_prompt_dir_finding_for_version(dir, installed_codex_version()?)
}

/// TASK-1124: rule-delivery-rot detection. Flags deployed vendor prompts/skills
/// that have drifted from the binary's embedded source templates, in two
/// places: (1) the project-local `.claude/`+`.codex/` scaffold (via
/// `check_scaffold_status`, which already ignores the dev-repo symlink layout
/// per BUG-917), and (2) the machine-global `~/.codex/prompts` (the stale-prompt
/// case that contributed to the TASK-1123 reviewer-bypass incident). Detection
/// only — each finding names the re-scaffold command, no auto-heal.
// trace:TASK-1124 | ai:claude
fn scan_scaffold_drift(
    project_root: &std::path::Path,
    store: &aida_core::RequirementsStore,
) -> Vec<DoctorFinding> {
    let mut findings = Vec::new();

    // (1) Project-local vendor prompts/skills (Template-category, AIDA-owned).
    let config = ScaffoldConfig::default();
    let db_path = project_root.join(".aida/cache.db");
    let status = check_scaffold_status(store, project_root, &config, &db_path);
    let is_vendor_prompt_or_skill = |p: &std::path::Path| {
        let s = p.to_string_lossy();
        s.starts_with(".claude/commands/")
            || s.starts_with(".claude/skills/")
            || s.starts_with(".codex/skills/")
    };
    let drifted: Vec<String> = status
        .modified
        .iter()
        .filter_map(|(p, _)| is_vendor_prompt_or_skill(p).then(|| p.to_string_lossy().into_owned()))
        .collect();
    // trace:BUG-1117 | ai:codex
    // Missing `.codex/skills/*` is scaffold drift too: Codex >=0.142 does not
    // discover the old ~/.codex/prompts pack as `$aida-*`, so absence of the
    // project-local skill surface leaves a codex-vendor project with no working
    // skill entry point.
    let missing_vendor_files: Vec<String> = status
        .missing
        .iter()
        .filter_map(|p| is_vendor_prompt_or_skill(p).then(|| p.to_string_lossy().into_owned()))
        .collect();
    let missing_codex_skill_files: Vec<&String> = missing_vendor_files
        .iter()
        .filter(|p| p.starts_with(".codex/skills/"))
        .collect();
    if !missing_codex_skill_files.is_empty() {
        findings.push(DoctorFinding {
            category: "scaffold-drift".to_string(),
            id: "scaffold-drift/codex-skills-missing".to_string(),
            summary: format!(
                ".codex/skills is missing AIDA skill files ({} missing); Codex uses this project-local surface for `$aida-*`",
                missing_codex_skill_files.len()
            ),
            action: "Run `aida scaffold upgrade` to create `.codex/skills/aida-*/SKILL.md`; then reopen Codex or run `/skills` and use `$aida-capture`.".to_string(),
            safe_heal: false,
        });
    }
    if !drifted.is_empty() {
        findings.push(DoctorFinding {
            category: "scaffold-drift".to_string(),
            id: "scaffold-drift/project".to_string(),
            summary: format!(
                "{} deployed vendor prompt/skill file(s) drifted from the source templates (stale scaffolding)",
                drifted.len()
            ),
            action: "aida scaffold upgrade   (or `aida scaffold diff` to inspect)".to_string(),
            safe_heal: false,
        });
    }

    // (2) Machine-global ~/.codex/prompts — the TASK-1123 incident case.
    if let Some(dir) = dirs::home_dir().map(|h| h.join(".codex").join("prompts")) {
        // BUG-1095: Codex CLI 0.142 does not discover this generated directory
        // as an interactive custom slash-command surface. Flag the overclaimed
        // installation state separately from stale-content drift.
        // trace:BUG-1095 | ai:codex
        if let Some(finding) = codex_ignores_prompt_dir_finding(&dir) {
            findings.push(finding);
        }
        let drifted_prompts = codex_prompts_drift(&dir);
        if !drifted_prompts.is_empty() {
            findings.push(DoctorFinding {
                category: "scaffold-drift".to_string(),
                id: "scaffold-drift/codex-prompts".to_string(),
                summary: format!(
                    "~/.codex/prompts is stale — {} prompt(s) drifted from the current source templates",
                    drifted_prompts.len()
                ),
                // TASK-1170: the edit-preserving refresh is the safe delivery
                // path — --force is no longer needed to pick up a fix, and it
                // would take your own edits with it.
                action: "aida scaffold refresh".to_string(),
                safe_heal: false,
            });
        }
    }

    findings
}

/// One guarded command shape and the latency it must stay under.
///
/// Per-shape rather than one global number, so guarding a second command is a
/// new row rather than a reshaped config.
///
/// ADR-53: `budget_ms` alone cannot express a single-call ceiling — a
/// proportion is invariant to the magnitude of its own outliers, and improves
/// as its denominator grows, so it can go quiet while the worst call gets
/// worse. `ceiling_ms` is the second, independent limb: ANY single call over
/// it trips the gate regardless of how healthy the proportion looks.
// trace:STORY-1422 trace:ADR-53 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PerformanceBudget {
    pub(crate) cmd: String,
    pub(crate) budget_ms: u64,
    pub(crate) ceiling_ms: Option<u64>,
}

/// Read `[performance.budgets]` — a table of command shape to millisecond
/// budget, in one of two forms:
///
/// ```toml
/// [performance.budgets]
/// show = 1000                                    # budget only, no ceiling
/// "queue list" = { budget_ms = 2000, ceiling_ms = 8000 }
/// ```
///
/// A TABLE rather than `show_budget_ms`-style keys because a command shape can
/// contain a space (`queue list`), which a key suffix cannot express without
/// mangling. Absent section means no guarded commands, which is not a failure —
/// a project that has not opted in has nothing to breach. `ceiling_ms` is
/// optional (ADR-53): a shape with no ceiling is judged on proportion alone,
/// same as before this limb existed.
// trace:STORY-1422 trace:ADR-53 | ai:claude
pub(crate) fn performance_budgets(cfg: Option<&toml::Value>) -> Vec<PerformanceBudget> {
    let Some(table) = cfg
        .and_then(|c| c.get("performance"))
        .and_then(|p| p.get("budgets"))
        .and_then(|b| b.as_table())
    else {
        return Vec::new();
    };
    let mut budgets: Vec<PerformanceBudget> = table
        .iter()
        .filter_map(|(cmd, value)| {
            if let Some(ms) = value.as_integer() {
                let budget_ms = (ms > 0).then_some(ms as u64)?;
                return Some(PerformanceBudget {
                    cmd: cmd.to_string(),
                    budget_ms,
                    ceiling_ms: None,
                });
            }
            let inline = value.as_table()?;
            let budget_ms = inline.get("budget_ms")?.as_integer().filter(|ms| *ms > 0)? as u64;
            let ceiling_ms = inline
                .get("ceiling_ms")
                .and_then(|v| v.as_integer())
                .filter(|ms| *ms > 0)
                .map(|ms| ms as u64);
            Some(PerformanceBudget {
                cmd: cmd.to_string(),
                budget_ms,
                ceiling_ms,
            })
        })
        .collect();
    budgets.sort_by(|a, b| a.cmd.cmp(&b.cmd));
    budgets
}

/// How much of a shape's recent traffic may exceed its budget before the gate
/// trips, and over what window.
///
/// A PROPORTION, not a median and not "any call over budget" — ruled after the
/// distribution was measured rather than assumed. The `show` population is
/// BIMODAL: 4,438 calls under 1s and a second cluster at 9-10s, sharing one
/// command name and indistinguishable by every field the usage log records.
/// Against that shape the obvious statistics all fail — the median sits at
/// ~665ms and never trips even with the regression live, p90 swings from 812ms
/// to 9653ms depending on the window, and "any call over budget" is true 18-21%
/// of the time including after a fix.
// trace:STORY-1422 | ai:claude
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PerformancePolicy {
    /// Tolerated fraction of over-budget calls, as a percentage.
    pub(crate) tolerated_pct: f64,
    /// How far back to look.
    pub(crate) window_hours: i64,
}

impl Default for PerformancePolicy {
    fn default() -> Self {
        // 10% over 24h, PROVISIONAL. It trips today at 20.3% with 2x margin and
        // clears a post-fix residual estimated at 7.1% with only 1.4x. That
        // second margin is thin and known to be thin: the 1s-4s band may be
        // partial-cache cases that vanish with the fix, or a genuine tail that
        // stays, and nothing in the data distinguishes those futures. The number
        // changes only alongside a re-measurement, never by tuning until green.
        Self {
            tolerated_pct: 10.0,
            window_hours: 24,
        }
    }
}

/// Read `[performance]` policy knobs, falling back to the provisional default.
// trace:STORY-1422 | ai:claude
pub(crate) fn performance_policy(cfg: Option<&toml::Value>) -> PerformancePolicy {
    let section = cfg.and_then(|c| c.get("performance"));
    let mut policy = PerformancePolicy::default();
    if let Some(pct) = section
        .and_then(|p| p.get("tolerated_over_budget_pct"))
        .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
        .filter(|pct| (0.0..=100.0).contains(pct))
    {
        policy.tolerated_pct = pct;
    }
    if let Some(hours) = section
        .and_then(|p| p.get("window_hours"))
        .and_then(|v| v.as_integer())
        .filter(|h| *h > 0)
    {
        policy.window_hours = hours;
    }
    policy
}

/// STORY-1367 default: the free-space floor a project has not configured
/// explicitly. Matches the number measured against the incident this job
/// exists to catch (root hit 100% with 16K free; a check at this floor would
/// have surfaced it hours earlier).
// trace:STORY-1367 | ai:claude
const DEFAULT_DISK_HEADROOM_MIN_FREE_GIB: u64 = 60;

/// Read `[doctor.disk_headroom] min_free_gib`, falling back to the default
/// floor. A project on a smaller or larger disk than the measured incident
/// tunes this rather than the check code.
// trace:STORY-1367 | ai:claude
pub(crate) fn disk_headroom_min_free_gib(cfg: Option<&toml::Value>) -> u64 {
    cfg.and_then(|c| c.get("doctor"))
        .and_then(|d| d.get("disk_headroom"))
        .and_then(|d| d.get("min_free_gib"))
        .and_then(|v| v.as_integer())
        .filter(|v| *v > 0)
        .map(|v| v as u64)
        .unwrap_or(DEFAULT_DISK_HEADROOM_MIN_FREE_GIB)
}

/// Pure threshold judgment, separated from the disk probe so the floor logic
/// is testable without manufacturing a full filesystem (same shape as
/// `machine_readiness::decide_capacity`). `None` = healthy = no finding — a
/// job that reports every run trains people to ignore it.
// trace:STORY-1367 | ai:claude
fn disk_headroom_finding(free_bytes: u64, min_free_gib: u64) -> Option<DoctorFinding> {
    let min_free_bytes = min_free_gib.saturating_mul(1024 * 1024 * 1024);
    if free_bytes >= min_free_bytes {
        return None;
    }
    let free_gib = free_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    Some(DoctorFinding {
        category: "disk-headroom".to_string(),
        id: "disk-headroom".to_string(),
        summary: format!(
            "free disk space is {free_gib:.1} GiB, below the {min_free_gib} GiB floor \
             (`[doctor.disk_headroom] min_free_gib`)"
        ),
        action: "reclaim space — stale worktrees (`aida session reap`), Rust build artifacts \
                 (`cargo clean`), or raise `min_free_gib` deliberately if the floor no longer \
                 matches this host"
            .to_string(),
        safe_heal: false,
    })
}

/// STORY-1367: free space on the filesystem holding `project_root`, via
/// `sysinfo` (same disk-enumeration approach as `machine_readiness`, which
/// this check does not reuse directly because that module answers a
/// different question — "is this host big enough for a planned drain of N
/// lanes/specs/hours" — while this one is an ambient, fixed-floor cadence
/// check with no drain-sizing inputs). Silent (empty) when the filesystem
/// can't be resolved rather than risk a false positive.
// trace:STORY-1367 | ai:claude
fn scan_disk_headroom(project_root: &std::path::Path, min_free_gib: u64) -> Vec<DoctorFinding> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let Some(disk) = disks
        .iter()
        .filter(|d| project_root.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
    else {
        return Vec::new();
    };
    disk_headroom_finding(disk.available_space(), min_free_gib)
        .into_iter()
        .collect()
}

/// BUG-1572: which binaries' calls the performance gate is allowed to count.
///
/// `~/.aida/usage.jsonl` is MACHINE-GLOBAL — every `aida` binary on the box
/// appends to it — and this repo routinely runs 20-60 git worktrees, each able
/// to build and run its own binary. Measured 2026-09-21, ELEVEN distinct
/// `binary_sha` values contributed to one 24h `show` window. So a single lane
/// on a stale build, or on a branch carrying a performance regression of its
/// own (precisely what such a lane is often built to investigate), could hold
/// MAIN's gate red indefinitely, and the aggregate finding gave the reading
/// seat no way to tell "main regressed" from "a lane is noisy".
///
/// The scope is ONE BINARY LINEAGE: ancestors of the current HEAD. That is the
/// same ancestry test `aida dev status` performs to decide whether the active
/// binary matches the branch, and it is reused rather than rewritten —
/// [`crate::current_branch_head_sha`] + [`crate::classify_sha_match`].
///
/// FAIL OPEN, NEVER CLOSED. Only a sha git POSITIVELY places off-lineage
/// ([`crate::ShaMatch::Unrelated`] — `merge-base --is-ancestor` exited a clean
/// `1` with both commits resolved) is excluded. A row with NO `binary_sha`, a
/// sha too short or too mangled for git to resolve, and a sha from another
/// project's repo or from a build whose commit was never pushed all classify
/// [`crate::ShaMatch::Unknown`] (exit `128`) and are COUNTED. Silently dropping
/// unplaceable rows would shrink the denominator, which is the very defect this
/// fixes; failing closed on them would make the gate unusable on any machine
/// whose log predates this repo.
// trace:BUG-1572 | ai:claude
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct BinaryLineage {
    /// Lowercased shas git positively placed OFF this HEAD's lineage.
    off_lineage: std::collections::BTreeSet<String>,
    /// `false` = count every row (not a git repo, git unavailable, or opted out).
    scoped: bool,
}

impl BinaryLineage {
    /// The pre-BUG-1572 behaviour: every row in the window counts.
    pub(crate) fn unscoped() -> Self {
        Self::default()
    }

    /// Scope to HEAD's lineage, excluding exactly the given shas.
    pub(crate) fn scoped<I: IntoIterator<Item = String>>(off_lineage: I) -> Self {
        Self {
            off_lineage: off_lineage
                .into_iter()
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
            scoped: true,
        }
    }

    pub(crate) fn is_scoped(&self) -> bool {
        self.scoped
    }

    /// Does a call served by `sha` count toward the gate's decision? Pure, so
    /// the fail-open rule is unit-testable without a git repo.
    pub(crate) fn counts(&self, sha: Option<&str>) -> bool {
        if !self.scoped {
            return true;
        }
        match sha {
            // Absent sha: unplaceable, so counted. See the fail-open note above.
            None => true,
            Some(s) => !self.off_lineage.contains(&s.trim().to_ascii_lowercase()),
        }
    }
}

/// Env opt-out for the BUG-1572 lineage scope — restores the pre-fix
/// count-every-binary population. Documented in `docs/environment-variables.md`.
// trace:BUG-1572 | ai:claude
pub(crate) const PERF_GATE_ALL_BINARIES_ENV: &str = "AIDA_PERF_GATE_ALL_BINARIES";

/// Pure: does this env value turn the lineage scope off?
// trace:BUG-1572 | ai:claude
pub(crate) fn perf_gate_scope_disabled_by(value: Option<&str>) -> bool {
    matches!(
        value.map(str::trim).unwrap_or(""),
        "1" | "true" | "yes" | "on"
    )
}

/// Normalize a row's `binary_sha` — an empty/whitespace field is the same as
/// an absent one.
// trace:BUG-1572 | ai:claude
fn event_binary_sha(ev: &crate::usage::UsageEvent) -> Option<&str> {
    ev.binary_sha
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Resolve the lineage scope for the guarded shapes' WINDOW population.
///
/// `repo` is `aida doctor`'s project root, which `main_worktree_root_from`
/// already resolves to the MAIN worktree — so the gate asks the same question
/// of the same repo from whichever of the 20-60 worktrees it is run in, and
/// two seats reading it get the same verdict. A lane's own unmerged build is
/// therefore off-lineage, which is the intended reading: this gate judges the
/// mainline, not whatever branch the caller happens to be standing on.
///
/// Deliberately narrowed to rows a budget actually guards that fall inside the
/// window: `read_events` returns the whole log, and classifying every sha it
/// has ever seen would fork one `git merge-base` per historical build. The
/// guarded window is ~10 distinct binaries.
// trace:BUG-1572 | ai:claude
pub(crate) fn resolve_binary_lineage(
    repo: &std::path::Path,
    events: &[crate::usage::UsageEvent],
    budgets: &[PerformanceBudget],
    policy: &PerformancePolicy,
    now: chrono::DateTime<chrono::Utc>,
) -> BinaryLineage {
    if perf_gate_scope_disabled_by(std::env::var(PERF_GATE_ALL_BINARIES_ENV).ok().as_deref()) {
        return BinaryLineage::unscoped();
    }
    // No HEAD means nothing to be an ancestor OF — run unscoped rather than
    // excluding everything.
    let Some(head) = crate::current_branch_head_sha(repo) else {
        return BinaryLineage::unscoped();
    };
    let cutoff = now - chrono::Duration::hours(policy.window_hours);
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for ev in events {
        if !budgets.iter().any(|b| b.cmd == ev.cmd) {
            continue;
        }
        if !usage_event_time(&ev.ts).is_some_and(|t| t >= cutoff) {
            continue;
        }
        if let Some(sha) = event_binary_sha(ev) {
            seen.insert(sha.to_ascii_lowercase());
        }
    }
    let off_lineage: Vec<String> = seen
        .into_iter()
        .filter(|sha| {
            matches!(
                crate::classify_sha_match(repo, sha, &head),
                crate::ShaMatch::Unrelated
            )
        })
        .collect();
    BinaryLineage::scoped(off_lineage)
}

/// The label used for a row whose `binary_sha` field is absent or blank.
pub(crate) const UNKNOWN_BINARY_LABEL: &str = "unknown";

/// One binary's slice of a guarded shape's window population.
///
/// BUG-1572: the finding carries these so a MIXED population is VISIBLE
/// instead of silently averaged. "Main regressed" and "one lane is noisy"
/// produce the same aggregate proportion; only the per-binary denominators
/// tell them apart, and the seat reading the gate should not have to
/// hand-write the grouping query that discovered the defect.
// trace:BUG-1572 | ai:claude
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BinaryTally {
    pub(crate) sha: String,
    pub(crate) samples: usize,
    pub(crate) over_budget: usize,
    pub(crate) over_pct: f64,
    /// Did this binary's calls count toward the gate's decision?
    pub(crate) counted: bool,
}

/// Group a shape's window population by `binary_sha`. Pure; sorted by sample
/// count descending then sha, so the breakdown is stable across runs.
// trace:BUG-1572 | ai:claude
pub(crate) fn tally_by_binary(
    window: &[(Option<&str>, u64)],
    budget_ms: u64,
    lineage: &BinaryLineage,
) -> Vec<BinaryTally> {
    let mut by_sha: std::collections::BTreeMap<String, (usize, usize, bool)> =
        std::collections::BTreeMap::new();
    for (sha, ms) in window {
        let key = sha
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_else(|| UNKNOWN_BINARY_LABEL.to_string());
        let entry = by_sha.entry(key).or_insert((0, 0, lineage.counts(*sha)));
        entry.0 += 1;
        if *ms > budget_ms {
            entry.1 += 1;
        }
    }
    let mut tallies: Vec<BinaryTally> = by_sha
        .into_iter()
        .map(|(sha, (samples, over_budget, counted))| BinaryTally {
            sha,
            samples,
            over_budget,
            over_pct: (over_budget as f64) * 100.0 / (samples as f64),
            counted,
        })
        .collect();
    tallies.sort_by(|a, b| b.samples.cmp(&a.samples).then(a.sha.cmp(&b.sha)));
    tallies
}

/// Render the per-binary denominators onto a breach summary.
///
/// Every binary is listed, counted and excluded alike — truncating the list
/// would re-hide exactly the mixed population BUG-1572 exists to expose.
// trace:BUG-1572 | ai:claude
pub(crate) fn binary_breakdown_phrase(per_binary: &[BinaryTally], lineage_scoped: bool) -> String {
    fn render(t: &BinaryTally) -> String {
        let short = t.sha.get(..t.sha.len().min(10)).unwrap_or(&t.sha);
        format!("{} {:.1}% of {}", short, t.over_pct, t.samples)
    }
    let counted: Vec<String> = per_binary
        .iter()
        .filter(|t| t.counted)
        .map(render)
        .collect();
    let excluded: Vec<&BinaryTally> = per_binary.iter().filter(|t| !t.counted).collect();
    let mut phrase = format!("; per binary — counted: {}", counted.join(", "));
    if !lineage_scoped {
        phrase.push_str(
            " (lineage scope OFF — this denominator spans every aida binary on the machine)",
        );
    }
    if !excluded.is_empty() {
        let n: usize = excluded.iter().map(|t| t.samples).sum();
        phrase.push_str(&format!(
            "; excluded {} calls from {} binar{} outside this HEAD's lineage: {}",
            n,
            excluded.len(),
            if excluded.len() == 1 { "y" } else { "ies" },
            excluded
                .into_iter()
                .map(render)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    phrase
}

/// ADR-53's ceiling limb, rendered onto a breach summary. Reports the
/// ceiling's presence and status even when it did NOT trip the gate, so a
/// reader can tell "no ceiling configured" from "ceiling configured and
/// clear" — the same absent-evidence-is-not-good-evidence rule the
/// unobserved-budget finding already follows.
// trace:ADR-53 | ai:claude
fn ceiling_clause(ceiling_ms: Option<u64>, worst_ms: u64, breached: bool) -> String {
    match ceiling_ms {
        Some(c) if breached => {
            format!("; CEILING BREACHED — a single call ran {worst_ms} ms against a {c} ms ceiling")
        }
        Some(c) => format!("; ceiling {c} ms not breached"),
        None => String::new(),
    }
}

/// One breached budget, kept machine-readable for the ledger.
///
/// `budget_ms` and `tolerated_pct` are both recorded because a CONFIGURED
/// threshold is a number someone can quietly raise the moment it trips — the
/// documented way latency gates die. A later reader must be able to tell a
/// fixed regression from a raised ceiling, and that is only possible if the
/// numbers in force AT THE TIME OF THE TRIP are in the record.
// trace:STORY-1422 | ai:claude
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PerformanceBreach {
    pub(crate) cmd: String,
    pub(crate) budget_ms: u64,
    pub(crate) over_budget: usize,
    pub(crate) samples: usize,
    pub(crate) over_pct: f64,
    pub(crate) tolerated_pct: f64,
    pub(crate) window_hours: i64,
    pub(crate) worst_ms: u64,
    /// BUG-1572: the FULL window population split by `binary_sha` — counted
    /// and excluded binaries alike — so a mixed population is visible rather
    /// than silently averaged into one proportion.
    pub(crate) per_binary: Vec<BinaryTally>,
    /// Calls in the window dropped because their binary is off this HEAD's
    /// lineage. `0` when the gate runs unscoped.
    pub(crate) excluded_samples: usize,
    /// Was the lineage scope ACTIVE for this decision? Recorded because
    /// "nothing was excluded" and "the scope was off" otherwise render
    /// identically, and only the second means the denominator is machine-wide.
    pub(crate) lineage_scoped: bool,
    /// ADR-53's second limb: the configured single-call ceiling, if any.
    pub(crate) ceiling_ms: Option<u64>,
    /// Did `worst_ms` exceed `ceiling_ms`? A breach can be recorded on this
    /// limb alone even while `over_pct` sits comfortably under `tolerated_pct`
    /// — that gap is the whole reason ADR-53 exists.
    pub(crate) ceiling_breached: bool,
}

/// Parse a usage event's timestamp, tolerating both `Z` and offset forms.
// trace:STORY-1422 | ai:claude
fn usage_event_time(ts: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

/// A guarded shape's window population: every in-window call, paired with the
/// binary that served it. Kept together because BUG-1572 needs BOTH the scoped
/// subset (the gate's decision) and the full split (the finding's breakdown).
// trace:BUG-1572 | ai:claude
fn window_population<'a>(
    events: &'a [crate::usage::UsageEvent],
    cmd: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> Vec<(Option<&'a str>, u64)> {
    events
        .iter()
        .filter(|ev| ev.cmd == cmd)
        .filter(|ev| usage_event_time(&ev.ts).is_some_and(|t| t >= cutoff))
        .map(|ev| (event_binary_sha(ev), ev.duration_ms))
        .collect()
}

/// Which guarded shapes exceed their budget TOO OFTEN inside the window.
///
/// Pure over its inputs INCLUDING `now` and `lineage`, so the decision is
/// testable without a usage log on disk, without a git repo, and without
/// depending on the wall clock.
///
/// BUG-1572: `lineage` scopes the DECISION population to one binary lineage.
/// The reported `per_binary` split still covers every binary in the window —
/// scoping the gate without showing what was scoped out would just relocate
/// the blind spot.
// trace:STORY-1422 trace:BUG-1572 | ai:claude
pub(crate) fn performance_breaches(
    events: &[crate::usage::UsageEvent],
    budgets: &[PerformanceBudget],
    policy: &PerformancePolicy,
    now: chrono::DateTime<chrono::Utc>,
    lineage: &BinaryLineage,
) -> Vec<PerformanceBreach> {
    let cutoff = now - chrono::Duration::hours(policy.window_hours);
    let mut breaches = Vec::new();
    for budget in budgets {
        let window = window_population(events, &budget.cmd, cutoff);
        let per_binary = tally_by_binary(&window, budget.budget_ms, lineage);
        let counted: Vec<u64> = window
            .iter()
            .filter(|(sha, _)| lineage.counts(*sha))
            .map(|(_, ms)| *ms)
            .collect();
        if counted.is_empty() {
            // No in-lineage samples in the window is NOT a pass —
            // `performance_unobserved` reports it (and says how many
            // off-lineage calls were dropped), so silence is explained
            // rather than assumed.
            continue;
        }
        let over_budget = counted.iter().filter(|ms| **ms > budget.budget_ms).count();
        let over_pct = (over_budget as f64) * 100.0 / (counted.len() as f64);
        let worst_ms = counted.iter().copied().max().unwrap_or(0);
        // ADR-53: a single call over the ceiling trips the gate on its own —
        // a proportion cannot express this, because it is invariant to the
        // magnitude of its own outliers and improves as its denominator grows.
        let ceiling_breached = budget.ceiling_ms.is_some_and(|c| worst_ms > c);
        if over_pct > policy.tolerated_pct || ceiling_breached {
            breaches.push(PerformanceBreach {
                cmd: budget.cmd.clone(),
                budget_ms: budget.budget_ms,
                over_budget,
                samples: counted.len(),
                over_pct,
                tolerated_pct: policy.tolerated_pct,
                window_hours: policy.window_hours,
                worst_ms,
                per_binary,
                excluded_samples: window.len() - counted.len(),
                lineage_scoped: lineage.is_scoped(),
                ceiling_ms: budget.ceiling_ms,
                ceiling_breached,
            });
        }
    }
    breaches
}

/// Guarded shapes that produced NO samples at all.
///
/// Reported separately and deliberately: a budget naming a command that never
/// ran is indistinguishable from a healthy one if both are silent, and a typo
/// in a shape name is exactly how a scheduled guard watches nothing for months
/// while reporting success. Absent evidence is not good evidence.
// trace:STORY-1422 | ai:claude
///
/// BUG-1572: "unobserved" is judged against the SCOPED population, because a
/// gate scoped to one binary lineage has no evidence about that lineage when
/// every in-window call came from another one. `excluded_samples` records how
/// many calls were dropped so the message can say WHY it is silent — a bare
/// "no recorded calls" would be false where 800 calls exist off-lineage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnobservedBudget {
    pub(crate) cmd: String,
    /// In-window calls dropped as off-lineage. `0` = genuinely no calls.
    pub(crate) excluded_samples: usize,
}

pub(crate) fn performance_unobserved(
    events: &[crate::usage::UsageEvent],
    budgets: &[PerformanceBudget],
    policy: &PerformancePolicy,
    now: chrono::DateTime<chrono::Utc>,
    lineage: &BinaryLineage,
) -> Vec<UnobservedBudget> {
    let cutoff = now - chrono::Duration::hours(policy.window_hours);
    budgets
        .iter()
        .filter_map(|budget| {
            let window = window_population(events, &budget.cmd, cutoff);
            let counted = window
                .iter()
                .filter(|(sha, _)| lineage.counts(*sha))
                .count();
            if counted > 0 {
                return None;
            }
            Some(UnobservedBudget {
                cmd: budget.cmd.clone(),
                excluded_samples: window.len(),
            })
        })
        .collect()
}

/// The `performance` category's findings: shapes over budget too often, plus
/// guarded shapes that produced no samples in the window.
///
/// BOTH ARE FINDINGS, and that is the load-bearing decision. The instrument
/// this category replaces could not distinguish "over budget", "under budget"
/// and "pointed at a command that does not exist" — three states, one exit
/// code, which is how a scheduled guard watches nothing for months and reports
/// health. An unobserved budget is a finding so silence has to be explained.
// trace:STORY-1422 | ai:claude
pub(crate) fn performance_findings(
    events: &[crate::usage::UsageEvent],
    budgets: &[PerformanceBudget],
    policy: &PerformancePolicy,
    now: chrono::DateTime<chrono::Utc>,
    lineage: &BinaryLineage,
) -> Vec<DoctorFinding> {
    let mut findings: Vec<DoctorFinding> =
        performance_breaches(events, budgets, policy, now, lineage)
            .into_iter()
            .map(|b| DoctorFinding {
                category: "performance".to_string(),
                id: b.cmd.clone(),
                summary: format!(
                    "`aida {}` exceeded its {} ms budget on {:.1}% of {} calls in the last {}h \
                 (tolerated {:.1}%, worst {} ms){}{}",
                    b.cmd,
                    b.budget_ms,
                    b.over_pct,
                    b.samples,
                    b.window_hours,
                    b.tolerated_pct,
                    b.worst_ms,
                    ceiling_clause(b.ceiling_ms, b.worst_ms, b.ceiling_breached),
                    binary_breakdown_phrase(&b.per_binary, b.lineage_scoped)
                ),
                action: if b.ceiling_breached {
                    "a single call exceeded the ceiling — that trips regardless of the \
                     proportion, because a proportion cannot express one bad call; \
                     investigate that call, or raise the ceiling deliberately in \
                     [performance.budgets] with the consequence in front of you"
                        .to_string()
                } else {
                    "investigate the regression, or change the budget deliberately in \
                 [performance] — raising a threshold the moment it trips is how a latency gate \
                 dies, so the numbers in force are recorded with the trip"
                        .to_string()
                },
                safe_heal: false,
            })
            .collect();

    findings.extend(
        performance_unobserved(events, budgets, policy, now, lineage)
            .into_iter()
            .map(|u| DoctorFinding {
                category: "performance".to_string(),
                id: u.cmd.clone(),
                summary: if u.excluded_samples > 0 {
                    // BUG-1572: say WHY it is silent. "No recorded calls" would
                    // be plainly false with hundreds of off-lineage calls in
                    // the window, and a reader would chase the wrong cause.
                    format!(
                        "`aida {}` has a budget but NO recorded calls from this HEAD's binary \
                         lineage in the last {}h — {} in-window call(s) were served by binaries \
                         outside it and were not counted",
                        u.cmd, policy.window_hours, u.excluded_samples
                    )
                } else {
                    format!(
                        "`aida {}` has a budget but NO recorded calls in the last {}h — the \
                         guard is watching a command it never sees",
                        u.cmd, policy.window_hours
                    )
                },
                action: if u.excluded_samples > 0 {
                    "run the guarded command on a binary built from this HEAD's lineage, or set \
                     AIDA_PERF_GATE_ALL_BINARIES=1 to count every binary on the machine"
                        .to_string()
                } else {
                    "check the shape spelling in [performance.budgets] and that telemetry \
                     is enabled; an unobserved budget proves nothing"
                        .to_string()
                },
                safe_heal: false,
            }),
    );

    findings
}

/// Typed, allow-listed evidence for the scheduler boundary. This deliberately
/// repeats the pure gate calculation rather than asking a consumer to parse a
/// human sentence. A zero denominator is an explicit record, not division.
// trace:BUG-1573 | ai:codex
fn performance_audit_records(
    events: &[crate::usage::UsageEvent],
    budgets: &[PerformanceBudget],
    policy: &PerformancePolicy,
    now: chrono::DateTime<chrono::Utc>,
    lineage: &BinaryLineage,
) -> Vec<schedule_ledger::PerformanceAudit> {
    let mut out: Vec<_> = performance_breaches(events, budgets, policy, now, lineage)
        .into_iter()
        .map(|b| schedule_ledger::PerformanceAudit {
            command: b.cmd,
            budget_ms: b.budget_ms,
            over_budget: b.over_budget,
            denominator: b.samples,
            proportion_millipercent: (b.over_pct * 1000.0).round() as u32,
            tolerated_millipercent: (b.tolerated_pct * 1000.0).round() as u32,
            window_hours: b.window_hours,
            worst_ms: Some(b.worst_ms),
            excluded_samples: b.excluded_samples,
            lineage_scoped: b.lineage_scoped,
            ceiling_ms: b.ceiling_ms,
            ceiling_breached: b.ceiling_breached,
        })
        .collect();
    for u in performance_unobserved(events, budgets, policy, now, lineage) {
        let Some(budget) = budgets.iter().find(|b| b.cmd == u.cmd) else {
            continue;
        };
        out.push(schedule_ledger::PerformanceAudit {
            command: u.cmd,
            budget_ms: budget.budget_ms,
            over_budget: 0,
            denominator: 0,
            proportion_millipercent: 0,
            tolerated_millipercent: (policy.tolerated_pct * 1000.0).round() as u32,
            window_hours: policy.window_hours,
            worst_ms: None,
            excluded_samples: u.excluded_samples,
            lineage_scoped: lineage.is_scoped(),
            ceiling_ms: budget.ceiling_ms,
            ceiling_breached: false,
        });
    }
    out.sort_by(|a, b| a.command.cmp(&b.command));
    out.truncate(schedule_ledger::MAX_PERFORMANCE_AUDITS);
    out
}

#[cfg(test)]
mod story_1422_performance_gate_tests {
    use super::*;

    /// Fixed clock: the window decision must not depend on when the suite runs.
    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-09-21T12:00:00+00:00")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    fn ev_at(cmd: &str, duration_ms: u64, hours_ago: i64) -> crate::usage::UsageEvent {
        crate::usage::UsageEvent {
            ts: (now() - chrono::Duration::hours(hours_ago)).to_rfc3339(),
            cmd: cmd.to_string(),
            args_count: 0,
            exit_code: 0,
            duration_ms,
            binary_sha: None,
            role: None,
            scope: None,
            schedule_source: None,
        }
    }

    fn budget(cmd: &str, budget_ms: u64) -> PerformanceBudget {
        PerformanceBudget {
            cmd: cmd.to_string(),
            budget_ms,
            ceiling_ms: None,
        }
    }

    /// `slow` of `total` calls over budget, all inside the window.
    fn mix(slow: usize, total: usize) -> Vec<crate::usage::UsageEvent> {
        (0..total)
            .map(|i| ev_at("show", if i < slow { 9_900 } else { 400 }, 1))
            .collect()
    }

    /// TODAY'S MEASURED POPULATION trips: 20.3% of calls over a 1000ms budget
    /// against the ruled 10% tolerance.
    // trace:STORY-1422 | ai:claude
    #[test]
    fn the_measured_population_trips_the_gate() {
        let breaches = performance_breaches(
            &mix(203, 1000),
            &[budget("show", 1000)],
            &PerformancePolicy::default(),
            now(),
            &BinaryLineage::unscoped(),
        );
        assert_eq!(breaches.len(), 1, "{breaches:?}");
        assert_eq!(breaches[0].over_budget, 203);
        assert_eq!(breaches[0].samples, 1000);
        assert!((breaches[0].over_pct - 20.3).abs() < 0.01);
    }

    /// THE ESTIMATED POST-FIX POPULATION MUST NOT TRIP. If the slow mode goes
    /// away, the residual 1s-4s band is ~7.1% of traffic. A 5% tolerance — the
    /// number this seat first proposed — would leave the gate permanently red
    /// AFTER the bug it guards was fixed. This test is why 5% was rejected, and
    /// it pins the rejected value so re-tightening is a deliberate act.
    // trace:STORY-1422 | ai:claude
    #[test]
    fn the_estimated_post_fix_population_does_not_trip() {
        let events = mix(71, 1000); // 7.1%
        let budgets = [budget("show", 1000)];
        assert!(
            performance_breaches(
                &events,
                &budgets,
                &PerformancePolicy::default(),
                now(),
                &BinaryLineage::unscoped()
            )
            .is_empty(),
            "7.1% must clear the ruled 10% tolerance"
        );
        let five_pct = PerformancePolicy {
            tolerated_pct: 5.0,
            window_hours: 24,
        };
        assert_eq!(
            performance_breaches(
                &events,
                &budgets,
                &five_pct,
                now(),
                &BinaryLineage::unscoped()
            )
            .len(),
            1,
            "and 5% would have tripped on it — the rejected threshold, pinned"
        );
    }

    /// The window is the filter, proved in both directions.
    // trace:STORY-1422 | ai:claude
    #[test]
    fn calls_outside_the_window_are_not_judged() {
        let mut events: Vec<_> = (0..100).map(|_| ev_at("show", 9_900, 48)).collect();
        events.extend((0..100).map(|_| ev_at("show", 400, 1)));
        let budgets = [budget("show", 1000)];
        assert!(
            performance_breaches(
                &events,
                &budgets,
                &PerformancePolicy::default(),
                now(),
                &BinaryLineage::unscoped()
            )
            .is_empty(),
            "48h-old breaches fall outside a 24h window"
        );
        let wide = PerformancePolicy {
            tolerated_pct: 10.0,
            window_hours: 72,
        };
        assert_eq!(
            performance_breaches(&events, &budgets, &wide, now(), &BinaryLineage::unscoped()).len(),
            1,
            "widening to 72h brings them back — so the window IS the filter"
        );
    }

    /// THE TYPO CASE, window-aware: a budget whose shape produced nothing
    /// RECENTLY is a finding, not silence.
    // trace:STORY-1422 | ai:claude
    #[test]
    fn a_budget_with_no_recent_calls_is_a_finding_not_a_pass() {
        let events = vec![ev_at("show", 400, 1)];
        let policy = PerformancePolicy::default();
        let typo = [budget("shwo", 1000)];
        let unscoped = BinaryLineage::unscoped();
        assert!(performance_breaches(&events, &typo, &policy, now(), &unscoped).is_empty());
        assert_eq!(
            performance_unobserved(&events, &typo, &policy, now(), &unscoped),
            vec![UnobservedBudget {
                cmd: "shwo".to_string(),
                excluded_samples: 0,
            }]
        );
        let findings = performance_findings(&events, &typo, &policy, now(), &unscoped);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert!(findings[0].summary.contains("NO recorded calls"));

        let stale = vec![ev_at("show", 9_900, 48)];
        assert_eq!(
            performance_unobserved(&stale, &[budget("show", 1000)], &policy, now(), &unscoped),
            vec![UnobservedBudget {
                cmd: "show".to_string(),
                excluded_samples: 0,
            }]
        );
    }

    /// The trip records the numbers IN FORCE, so a later reader can tell a
    /// fixed regression from a raised ceiling.
    // trace:STORY-1422 | ai:claude
    #[test]
    fn a_trip_records_the_budget_and_tolerance_in_force() {
        let policy = PerformancePolicy {
            tolerated_pct: 10.0,
            window_hours: 24,
        };
        let events = mix(300, 1000);
        let budgets = [budget("show", 1234)];
        let b = &performance_breaches(
            &events,
            &budgets,
            &policy,
            now(),
            &BinaryLineage::unscoped(),
        )[0];
        assert_eq!(b.budget_ms, 1234);
        assert_eq!(b.tolerated_pct, 10.0);
        assert_eq!(b.window_hours, 24);
        assert_eq!(b.worst_ms, 9_900);
        let summary = &performance_findings(
            &events,
            &budgets,
            &policy,
            now(),
            &BinaryLineage::unscoped(),
        )[0]
        .summary;
        assert!(summary.contains("1234 ms"), "{summary}");
        assert!(summary.contains("tolerated 10.0%"), "{summary}");
    }

    /// Per-shape budgets, including a shape containing a space.
    // trace:STORY-1422 | ai:claude
    #[test]
    fn budgets_are_per_shape_and_tolerate_spaces() {
        let cfg: toml::Value = "[performance.budgets]\nshow = 1000\n\"queue list\" = 2000\n"
            .parse()
            .unwrap();
        assert_eq!(
            performance_budgets(Some(&cfg)),
            vec![budget("queue list", 2000), budget("show", 1000)]
        );
        assert!(performance_budgets(None).is_empty());
    }

    /// A zero or non-integer budget is ignored rather than treated as "every
    /// call breaches", so a malformed config cannot manufacture findings.
    // trace:STORY-1422 | ai:claude
    #[test]
    fn malformed_budgets_are_ignored_not_treated_as_zero() {
        let cfg: toml::Value = "[performance.budgets]\nshow = 0\nlist = \"fast\"\n"
            .parse()
            .unwrap();
        assert!(performance_budgets(Some(&cfg)).is_empty());
    }

    /// Policy is configurable; absent or out-of-range values fall back to the
    /// provisional default rather than silently disabling the gate.
    // trace:STORY-1422 | ai:claude
    #[test]
    fn policy_falls_back_to_the_provisional_default() {
        assert_eq!(performance_policy(None), PerformancePolicy::default());
        let ok: toml::Value = "[performance]\ntolerated_over_budget_pct = 25\nwindow_hours = 6\n"
            .parse()
            .unwrap();
        let p = performance_policy(Some(&ok));
        assert_eq!(p.tolerated_pct, 25.0);
        assert_eq!(p.window_hours, 6);
        let bad: toml::Value = "[performance]\ntolerated_over_budget_pct = 900\nwindow_hours = 0\n"
            .parse()
            .unwrap();
        assert_eq!(performance_policy(Some(&bad)), PerformancePolicy::default());
    }

    /// ADR-53: `ceiling_ms` parses from the table form, and the plain-integer
    /// form still parses with `ceiling_ms: None` — the pre-ADR-53 config keeps
    /// working unchanged.
    // trace:ADR-53 | ai:claude
    #[test]
    fn ceiling_ms_parses_from_the_table_form_and_defaults_to_none() {
        let cfg: toml::Value =
            "[performance.budgets]\nshow = { budget_ms = 1000, ceiling_ms = 5000 }\n\"queue list\" = 2000\n"
                .parse()
                .unwrap();
        let budgets = performance_budgets(Some(&cfg));
        assert_eq!(
            budgets,
            vec![
                budget("queue list", 2000),
                PerformanceBudget {
                    cmd: "show".to_string(),
                    budget_ms: 1000,
                    ceiling_ms: Some(5000),
                },
            ]
        );
    }

    /// A zero/non-positive `ceiling_ms` is ignored (treated as unset) rather
    /// than manufacturing a ceiling that trips on every call — same rule as a
    /// malformed `budget_ms`.
    // trace:ADR-53 | ai:claude
    #[test]
    fn a_malformed_ceiling_ms_is_ignored_not_treated_as_zero() {
        let cfg: toml::Value =
            "[performance.budgets]\nshow = { budget_ms = 1000, ceiling_ms = 0 }\n"
                .parse()
                .unwrap();
        assert_eq!(performance_budgets(Some(&cfg)), vec![budget("show", 1000)]);
    }

    fn budget_with_ceiling(cmd: &str, budget_ms: u64, ceiling_ms: u64) -> PerformanceBudget {
        PerformanceBudget {
            cmd: cmd.to_string(),
            budget_ms,
            ceiling_ms: Some(ceiling_ms),
        }
    }

    /// ADR-53's whole point: a single call over the ceiling trips the gate
    /// even while the PROPORTION sits comfortably under tolerance — the exact
    /// gap the measured 2.9%-at-n=170 / 69,240ms-worst-call reading exposed.
    // trace:ADR-53 | ai:claude
    #[test]
    fn a_single_call_over_the_ceiling_trips_the_gate_even_with_a_healthy_proportion() {
        // 1 of 170 over budget — 0.6%, nowhere near the 10% tolerance — but
        // that one call is a 69,240ms outlier, over the 5000ms ceiling.
        let mut events: Vec<_> = (0..169).map(|_| ev_at("show", 400, 1)).collect();
        events.push(ev_at("show", 69_240, 1));
        let budgets = [budget_with_ceiling("show", 1000, 5_000)];
        let breaches = performance_breaches(
            &events,
            &budgets,
            &PerformancePolicy::default(),
            now(),
            &BinaryLineage::unscoped(),
        );
        assert_eq!(breaches.len(), 1, "{breaches:?}");
        assert!(
            breaches[0].over_pct < PerformancePolicy::default().tolerated_pct,
            "the proportion limb alone must NOT explain this trip: {}",
            breaches[0].over_pct
        );
        assert!(breaches[0].ceiling_breached);
        assert_eq!(breaches[0].worst_ms, 69_240);

        let summary = &performance_findings(
            &events,
            &budgets,
            &PerformancePolicy::default(),
            now(),
            &BinaryLineage::unscoped(),
        )[0]
        .summary;
        assert!(summary.contains("CEILING BREACHED"), "{summary}");
        assert!(summary.contains("69240 ms"), "{summary}");
        assert!(summary.contains("5000 ms ceiling"), "{summary}");
    }

    /// The mirror image: proportion AND ceiling both clear must NOT trip —
    /// a configured-but-unbreached ceiling is not itself a finding.
    // trace:ADR-53 | ai:claude
    #[test]
    fn a_configured_ceiling_that_is_not_breached_does_not_trip() {
        let events = mix(29, 1000); // 2.9% over budget, worst call 9,900ms
        let budgets = [budget_with_ceiling("show", 1000, 15_000)];
        assert!(
            performance_breaches(
                &events,
                &budgets,
                &PerformancePolicy::default(),
                now(),
                &BinaryLineage::unscoped()
            )
            .is_empty(),
            "2.9% clears the 10% tolerance and the 9,900ms worst call clears a 15,000ms ceiling"
        );
    }

    /// STORY-1423 criterion 1 (ADR-53's "quiet on both limbs" amendment): a
    /// dogfood regression test over this repo's OWN `.aida/config.toml`, not a
    /// fixture. Every prior confirmation that the `show` budget carries a
    /// `ceiling_ms` was a point-in-time reading (`aida show --json` on some
    /// date); nothing stopped a later config edit from quietly dropping the
    /// table form back to a plain integer and losing the second limb without
    /// any test failing. Reads the real file via `CARGO_MANIFEST_DIR` so this
    /// fails the moment the dogfood surface regresses.
    // trace:STORY-1423 trace:ADR-53 | ai:claude
    #[test]
    fn this_repo_configures_a_ceiling_for_the_show_budget() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("aida-cli-lib has a workspace parent");
        let body = std::fs::read_to_string(repo_root.join(".aida/config.toml"))
            .expect("this repo's .aida/config.toml must be readable");
        let cfg: toml::Value = body
            .parse()
            .expect("this repo's .aida/config.toml must be valid TOML");
        let budgets = performance_budgets(Some(&cfg));
        let show = budgets
            .iter()
            .find(|b| b.cmd == "show")
            .expect("this repo must declare a [performance.budgets] entry for `show`");
        assert_eq!(
            show.budget_ms, 1000,
            "budget_ms must stay the measured value"
        );
        assert!(
            show.ceiling_ms.is_some(),
            "the `show` budget must keep ADR-53's ceiling_ms limb, not regress to proportion-only"
        );
    }
}

/// STORY-1367: the disk-headroom job. `disk_headroom_finding` is pure so the
/// floor logic is verified without manufacturing a full filesystem;
/// `scan_disk_headroom` is verified against the real filesystem with a floor
/// forced to each side of "always healthy" / "never healthy" so the wiring
/// itself (path → mount → available_space) is proven too.
#[cfg(test)]
mod story_1367_disk_headroom_tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn healthy_headroom_is_silent() {
        assert!(disk_headroom_finding(100 * GIB, 60).is_none());
        assert!(disk_headroom_finding(60 * GIB, 60).is_none());
    }

    #[test]
    fn low_headroom_reports_once_with_the_floor_and_measured_free_space() {
        let finding = disk_headroom_finding(16 * 1024, 60).expect("below-floor must report");
        assert_eq!(finding.category, "disk-headroom");
        assert!(finding.summary.contains("60 GiB"), "{}", finding.summary);
        assert!(!finding.safe_heal, "never auto-healed — operator decision");
    }

    #[test]
    fn min_free_gib_reads_config_and_falls_back_to_the_default() {
        assert_eq!(
            disk_headroom_min_free_gib(None),
            DEFAULT_DISK_HEADROOM_MIN_FREE_GIB
        );
        let cfg: toml::Value = "[doctor.disk_headroom]\nmin_free_gib = 200\n"
            .parse()
            .unwrap();
        assert_eq!(disk_headroom_min_free_gib(Some(&cfg)), 200);
        // A non-positive override is ignored rather than manufacturing an
        // always-failing (0) or always-passing check.
        let zero: toml::Value = "[doctor.disk_headroom]\nmin_free_gib = 0\n"
            .parse()
            .unwrap();
        assert_eq!(
            disk_headroom_min_free_gib(Some(&zero)),
            DEFAULT_DISK_HEADROOM_MIN_FREE_GIB
        );
    }

    #[test]
    fn scan_is_silent_against_a_floor_the_real_disk_always_clears() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(scan_disk_headroom(tmp.path(), 0).is_empty());
    }

    #[test]
    fn scan_reports_against_a_floor_no_real_disk_clears() {
        let tmp = tempfile::tempdir().unwrap();
        let findings = scan_disk_headroom(tmp.path(), u64::MAX / (1024 * 1024 * 1024));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, "disk-headroom");
    }
}

/// BUG-1572: the gate reads a MACHINE-GLOBAL log, so its population must be
/// scoped to one binary lineage and its finding must show the per-binary split.
#[cfg(test)]
mod bug_1572_binary_lineage_tests {
    use super::*;

    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-09-21T12:00:00+00:00")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    fn ev(cmd: &str, duration_ms: u64, sha: Option<&str>) -> crate::usage::UsageEvent {
        crate::usage::UsageEvent {
            ts: (now() - chrono::Duration::hours(1)).to_rfc3339(),
            cmd: cmd.to_string(),
            args_count: 0,
            exit_code: 0,
            duration_ms,
            binary_sha: sha.map(|s| s.to_string()),
            role: None,
            scope: None,
            schedule_source: None,
        }
    }

    fn budget(cmd: &str, budget_ms: u64) -> PerformanceBudget {
        PerformanceBudget {
            cmd: cmd.to_string(),
            budget_ms,
            ceiling_ms: None,
        }
    }

    /// `slow` of `total` `show` calls over a 1000ms budget, all served by `sha`.
    fn calls(sha: &str, slow: usize, total: usize) -> Vec<crate::usage::UsageEvent> {
        (0..total)
            .map(|i| ev("show", if i < slow { 9_900 } else { 400 }, Some(sha)))
            .collect()
    }

    /// THE DEFECT, BOTH DIRECTIONS. One window, two binaries: `inlineage` under
    /// budget and `otherlane` far over it. Scoped to HEAD's lineage the gate
    /// must stay QUIET — a noisy lane cannot hold main red. Flip WHICH binary
    /// is slow and the same window MUST trip — otherwise the scope is not a
    /// discriminator, it is just a mute button. One direction is half a test.
    // trace:BUG-1572 | ai:claude
    #[test]
    fn a_noisy_off_lineage_binary_does_not_trip_the_in_lineage_gate_but_a_noisy_in_lineage_one_does(
    ) {
        let budgets = [budget("show", 1000)];
        let policy = PerformancePolicy::default();
        let scoped = BinaryLineage::scoped(["otherlane".to_string()]);

        // Direction 1: in-lineage clean (5%), off-lineage filthy (80%).
        let mut mixed = calls("inlineage", 5, 100);
        mixed.extend(calls("otherlane", 80, 100));
        assert!(
            performance_breaches(&mixed, &budgets, &policy, now(), &scoped).is_empty(),
            "a lane's own noisy build must not hold this HEAD's gate red"
        );
        // …and the un-scoped population WOULD have tripped, so the scope is
        // what quiets it — not an accidentally-clean fixture.
        assert_eq!(
            performance_breaches(&mixed, &budgets, &policy, now(), &BinaryLineage::unscoped())
                .len(),
            1,
            "unscoped, the SAME window trips at 42.5% — that is the defect"
        );

        // Direction 2: swap which binary is slow. Same shape, same counts.
        let mut regressed = calls("inlineage", 80, 100);
        regressed.extend(calls("otherlane", 5, 100));
        let breaches = performance_breaches(&regressed, &budgets, &policy, now(), &scoped);
        assert_eq!(
            breaches.len(),
            1,
            "a real regression ON THIS LINEAGE must still trip"
        );
        assert_eq!(breaches[0].samples, 100, "denominator is the scoped set");
        assert_eq!(breaches[0].over_budget, 80);
        assert_eq!(breaches[0].excluded_samples, 100);
    }

    /// The emitted finding reports the denominator PER BINARY, counted and
    /// excluded alike — the mixed population is visible instead of averaged.
    // trace:BUG-1572 | ai:claude
    #[test]
    fn the_finding_reports_the_denominator_per_binary() {
        let mut events = calls("inlineage", 80, 100);
        events.extend(calls("otherlane", 5, 40));
        let findings = performance_findings(
            &events,
            &[budget("show", 1000)],
            &PerformancePolicy::default(),
            now(),
            &BinaryLineage::scoped(["otherlane".to_string()]),
        );
        assert_eq!(findings.len(), 1, "{findings:?}");
        let s = &findings[0].summary;
        assert!(s.contains("80.0% of 100 calls"), "aggregate stays: {s}");
        assert!(s.contains("counted: inlineage 80.0% of 100"), "{s}");
        assert!(
            s.contains("excluded 40 calls from 1 binary outside this HEAD's lineage"),
            "{s}"
        );
        assert!(s.contains("otherlane 12.5% of 40"), "{s}");
    }

    /// FAIL OPEN on a sha that cannot be placed. A row with no `binary_sha`,
    /// and a sha git never classified (short, purged, or from another
    /// project's repo), are COUNTED — only a positively off-lineage sha is
    /// dropped. Silently dropping unplaceable rows would shrink the
    /// denominator, which is the defect being fixed; failing closed on them
    /// would make the gate unusable.
    // trace:BUG-1572 | ai:claude
    #[test]
    fn unplaceable_and_absent_shas_are_counted_not_dropped() {
        let scoped = BinaryLineage::scoped(["otherlane".to_string()]);
        assert!(scoped.counts(None), "absent binary_sha counts");
        assert!(
            scoped.counts(Some("abc")),
            "a short/unresolvable sha counts"
        );
        assert!(
            scoped.counts(Some("deadbeefdeadbeef")),
            "a sha from another project's repo counts"
        );
        assert!(
            !scoped.counts(Some("otherlane")),
            "only proven off-lineage drops"
        );
        assert!(
            scoped.counts(Some("INLINEAGE")) && !scoped.counts(Some("OTHERLANE")),
            "sha matching is case-insensitive in both directions"
        );

        // …and it shows up in the population, not just the predicate.
        let mut events = calls("otherlane", 10, 10);
        events.push(ev("show", 9_900, None));
        events.push(ev("show", 9_900, Some("abc")));
        let breaches = performance_breaches(
            &events,
            &[budget("show", 1000)],
            &PerformancePolicy::default(),
            now(),
            &scoped,
        );
        assert_eq!(breaches.len(), 1, "{breaches:?}");
        assert_eq!(
            breaches[0].samples, 2,
            "the unplaceable rows form the denominator"
        );
    }

    /// An unscoped lineage is byte-for-byte the pre-BUG-1572 behaviour, so the
    /// no-git-repo / opted-out path cannot silently change a verdict.
    // trace:BUG-1572 | ai:claude
    #[test]
    fn an_unscoped_lineage_counts_every_binary() {
        let unscoped = BinaryLineage::unscoped();
        assert!(!unscoped.is_scoped());
        assert!(unscoped.counts(None) && unscoped.counts(Some("anything")));
        let mut events = calls("inlineage", 0, 100);
        events.extend(calls("otherlane", 100, 100));
        let b = performance_breaches(
            &events,
            &[budget("show", 1000)],
            &PerformancePolicy::default(),
            now(),
            &unscoped,
        );
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].samples, 200);
        assert_eq!(b[0].excluded_samples, 0);
        assert!(!b[0].lineage_scoped);
        assert!(
            binary_breakdown_phrase(&b[0].per_binary, b[0].lineage_scoped)
                .contains("lineage scope OFF"),
            "an unscoped denominator must SAY it is machine-wide"
        );
    }

    /// A shape whose every in-window call came from off-lineage binaries is
    /// UNOBSERVED, and the finding says so — "no recorded calls" would be
    /// plainly false with calls sitting in the window, and would send a reader
    /// hunting a telemetry outage instead of a binary mismatch.
    // trace:BUG-1572 | ai:claude
    #[test]
    fn an_all_off_lineage_window_reports_why_it_is_silent() {
        let events = calls("otherlane", 50, 60);
        let budgets = [budget("show", 1000)];
        let policy = PerformancePolicy::default();
        let scoped = BinaryLineage::scoped(["otherlane".to_string()]);
        assert_eq!(
            performance_unobserved(&events, &budgets, &policy, now(), &scoped),
            vec![UnobservedBudget {
                cmd: "show".to_string(),
                excluded_samples: 60,
            }]
        );
        let findings = performance_findings(&events, &budgets, &policy, now(), &scoped);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert!(
            findings[0]
                .summary
                .contains("60 in-window call(s) were served"),
            "{}",
            findings[0].summary
        );
    }

    /// The opt-out env value is parsed, not guessed.
    // trace:BUG-1572 | ai:claude
    #[test]
    fn the_opt_out_recognizes_only_affirmative_values() {
        for on in ["1", "true", "yes", "on", " 1 "] {
            assert!(perf_gate_scope_disabled_by(Some(on)), "{on}");
        }
        for off in ["0", "false", "", "no", "off", "maybe"] {
            assert!(!perf_gate_scope_disabled_by(Some(off)), "{off}");
        }
        assert!(!perf_gate_scope_disabled_by(None));
    }

    /// The tally is ordered by sample count descending, so the breakdown is
    /// stable across runs and the biggest contributor reads first.
    // trace:BUG-1572 | ai:claude
    #[test]
    fn the_per_binary_tally_is_stable_and_labels_absent_shas() {
        let window: Vec<(Option<&str>, u64)> = vec![
            (Some("aaa"), 9_900),
            (Some("bbb"), 400),
            (Some("bbb"), 400),
            (Some("bbb"), 9_900),
            (None, 400),
        ];
        let tallies = tally_by_binary(&window, 1000, &BinaryLineage::scoped(["bbb".to_string()]));
        assert_eq!(
            tallies
                .iter()
                .map(|t| (t.sha.as_str(), t.samples, t.over_budget, t.counted))
                .collect::<Vec<_>>(),
            vec![
                ("bbb", 3, 1, false),
                ("aaa", 1, 1, true),
                (UNKNOWN_BINARY_LABEL, 1, 0, true),
            ],
            "biggest contributor first, then sha — and \"aaa\" sorts before the \
             absent-sha label, so the tie-break is by name, not by insertion order"
        );
    }
}

/// BUG-1505: surface every review-verdict file that is not in the canonical
/// shape. An UNKNOWN / ambiguous verdict word (e.g. a qualified approval such
/// as `APPROVED pending cross-platform green`) gets its OWN finding: every
/// gate reads it as not-approved, and only a human can say what it meant.
/// Everything else (a re-spelling, a legacy key, a missing sha) is folded into
/// one aggregate finding so a large legacy corpus does not drown the report.
// trace:BUG-1505 | ai:claude
fn scan_review_verdicts(project_root: &std::path::Path) -> Vec<DoctorFinding> {
    let rows = crate::review_verdict::audit_verdict_dir(project_root);
    let mut out = Vec::new();
    let mut drifted = Vec::new();
    for row in &rows {
        if row.kind == crate::review_verdict::VerdictKind::Unknown && !row.raw.is_empty() {
            out.push(DoctorFinding {
                category: "review-verdicts".to_string(),
                id: format!("unknown-verdict:{}", row.file),
                summary: format!(
                    "{} records verdict `{}`, which is not a recognised verdict — every gate reads it as NOT approved",
                    row.file, row.raw
                ),
                action: format!(
                    "decide it by hand, then record a fresh verdict: `aida review record <SPEC> --verdict approved|request-changes|rejected --summary \"<why>\"` (file: .aida/review-verdicts/{})",
                    row.file
                ),
                safe_heal: false,
            });
        } else {
            drifted.push(row.file.as_str());
        }
    }
    if !drifted.is_empty() {
        let shown: Vec<&str> = drifted.iter().take(8).copied().collect();
        let more = drifted.len().saturating_sub(shown.len());
        let tail = if more > 0 {
            format!(" (+{more} more)")
        } else {
            String::new()
        };
        out.push(DoctorFinding {
            category: "review-verdicts".to_string(),
            id: "non-canonical-verdicts".to_string(),
            summary: format!(
                "{} review-verdict file(s) are not in the canonical shape (spelling, legacy keys, or no reviewed sha): {}{tail}",
                drifted.len(),
                shown.join(", ")
            ),
            action: "report only — readers normalize these; a shaless verdict is treated as absent. Re-record a verdict to write the canonical shape".to_string(),
            safe_heal: false,
        });
    }
    out
}

/// TASK-1122: store-scrub detection. When identity redaction is configured
/// (`[node] public_email` / `public_hostname` in `~/.aida/config.toml`), verify
/// the RAW system identity has not ALREADY leaked into the store — a leak that
/// landed before redaction was enabled is otherwise invisible on a public
/// mirror. Reads the identity-bearing store files + recent store commit authors
/// and flags any raw value present. Detection only, no auto-heal (removing an
/// already-landed value needs a history rewrite).
// trace:TASK-1122 | ai:claude
// trace:BUG-1561 | ai:claude
fn scan_store_scrub(project_root: &std::path::Path) -> Vec<DoctorFinding> {
    let (pub_host, pub_email) = aida_core::git_ops::public_identity();
    // Redaction not configured → nothing is expected to be redacted, nothing to check.
    if pub_host.is_none() && pub_email.is_none() {
        return Vec::new();
    }

    // The raw identity this machine writes.
    let raw_host = hostname();
    let raw_host = (!raw_host.is_empty()).then_some(raw_host);
    let raw_email = git_config_value(project_root, "user.email");

    // Gather the store text that carries identity: the registration + block
    // files, plus recent store commit authors on the orphan branch.
    let store_dir = project_root.join(".aida-store");
    let mut store_text = String::new();
    for f in ["nodes.toml", "blocks.yaml", "oplog.yaml"] {
        if let Ok(s) = std::fs::read_to_string(store_dir.join(f)) {
            store_text.push_str(&s);
            store_text.push('\n');
        }
    }
    if let Ok(out) = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["log", "aida-store", "--format=%ae%n%an", "-n", "1000"])
        .output()
    {
        if out.status.success() {
            store_text.push_str(&String::from_utf8_lossy(&out.stdout));
        }
    }

    let leaks = aida_core::git_ops::detect_identity_leaks(
        &store_text,
        raw_host.as_deref(),
        raw_email.as_deref(),
        pub_host.as_deref(),
        pub_email.as_deref(),
    );
    leaks
        .into_iter()
        .enumerate()
        .map(|(i, leak)| DoctorFinding {
            category: "store-scrub".to_string(),
            id: format!("store-scrub/{i}"),
            summary: format!("raw machine identity already in the store — {leak}"),
            action:
                "landed before redaction was enabled; scrub it from the store history before it propagates (see docs/security/)"
                    .to_string(),
            safe_heal: false,
        })
        .collect()
}

// TASK-1313: round-trip artifacts in stored spec text. A seat that captures
// `aida show` output (TOON-rendered when piped) and writes it back through a
// description edit corrupts the field two ways: the TOON rendering's row
// headers — and everything the renderer emitted after them — get swallowed
// into the value, and/or the capture path decodes UTF-8 as Latin-1 and
// mangles non-ASCII bytes into mojibake. Both predicates are measured against
// the real store in TASK-1313's own spec text (six genuine instances, zero
// false positives). REPORT-ONLY: this never rewrites a spec. Reconstructing a
// swallowed block means recovering the pre-corruption text from the orphan
// branch's git history, a per-spec judgment call for a human/reviewer, not an
// automated rewrite (that repair pass is TASK-1314). trace:TASK-1313 | ai:claude
fn round_trip_toon_header_regex() -> regex::Regex {
    // Anchored at column 0 of a line (`(?m)^`). Anchoring is what separates a
    // genuine swallowed TOON rendering — which always starts at a line's
    // first column, because that is where the renderer emits it — from prose
    // that merely discusses the convention mid-line. Unanchored, this very
    // check's own spec text (which quotes the pattern) would false-positive.
    regex::Regex::new(r"(?m)^(?:(?:relationships|next)\[\d+\]\{|execution_mode:)")
        .expect("valid TOON round-trip header regex")
}

fn round_trip_toon_header_regex_unanchored() -> regex::Regex {
    // Same alternation as `round_trip_toon_header_regex`, minus the `(?m)^`
    // anchor. Criterion 7f: a capture path that strips (or never had) the
    // trailing newline before the header lands it MID-LINE, a shape the
    // anchored regex cannot see by construction — the six observed real
    // leaks all happen to be line-anchored (one producer), which proves that
    // producer's behaviour, not the next one's. This unanchored form is kept
    // as a SEPARATE, lower-confidence pass rather than folded into the
    // primary predicate: unanchored also matches ordinary inline prose that
    // merely discusses the convention (four known false positives measured
    // against this store, including this very spec's own text), so
    // `scan_round_trip_artifacts` only reports an unanchored match when it
    // is NOT also an anchored one (i.e. genuinely mid-line), and tags it with
    // a distinct category/summary so it never dilutes the high-confidence
    // anchored findings.
    regex::Regex::new(r"(?:(?:relationships|next)\[\d+\]\{|execution_mode:)")
        .expect("valid TOON round-trip header regex (unanchored)")
}

fn round_trip_mojibake_regex() -> regex::Regex {
    // A lead character in {Â U+00C2, Ã U+00C3, â U+00E2} immediately followed
    // by EITHER a Latin-1 Supplement code point (U+0080-U+00FF — this is what
    // a raw UTF-8-as-Latin-1 mis-decode of a multi-byte sequence parses to)
    // OR a literal `\xHH` escape (the form a YAML double-quoted scalar stores
    // when the same damage still carries its escape text rather than having
    // been parsed into the control character). Scanning the PARSED field
    // value needs both alternatives, because YAML's parser already turns some
    // `\xHH` escapes into the control character and leaves others literal
    // depending on how many re-serialization passes the field went through.
    // A LONE Â/Ã/â is an ordinary letter — only the pair indicates a
    // mis-decode, so no bare-lead-character alternative is included.
    regex::Regex::new(r"[\u{00C2}\u{00C3}\u{00E2}](?:[\u{0080}-\u{00FF}]|\\x[0-9a-fA-F]{2})")
        .expect("valid mojibake regex")
}

/// Recursively collects `(field-label, content)` pairs for a comment list and
/// every nested reply beneath it (`Comment.replies` is itself
/// `Vec<Comment>` — a reply can carry its own replies). Criterion 6 requires
/// coverage of every text field, and a reply's content is exactly as capable
/// of carrying a swallowed TOON block or mojibake as a top-level comment's —
/// the round-trip producer captures rendered output regardless of comment
/// nesting depth. `prefix` is the field label of the comment list's parent
/// ("" for the requirement's own top-level `comments`, or a comment's own
/// field label when descending into its `replies`), so a reply's label reads
/// `comment[0]/reply[0]`, a reply-of-a-reply `comment[0]/reply[0]/reply[0]`,
/// and so on.
// trace:TASK-1313 | ai:claude
fn push_comment_fields<'a>(
    comments: &'a [aida_core::models::Comment],
    prefix: &str,
    out: &mut Vec<(String, &'a str)>,
) {
    for (i, c) in comments.iter().enumerate() {
        let field = if prefix.is_empty() {
            format!("comment[{i}]")
        } else {
            format!("{prefix}/reply[{i}]")
        };
        out.push((field.clone(), c.content.as_str()));
        push_comment_fields(&c.replies, &field, out);
    }
}

/// Whether the byte at `start` in `bytes` sits at the start of a line,
/// ALLOWING leading indentation (spaces/tabs) between the preceding newline
/// (or start of text) and `start`. Used only to decide what the unanchored
/// mid-line pass should SKIP as a duplicate of the anchored primary check's
/// intent: an indented code block quoting a TOON sample (own test:
/// `indented_toon_sample_is_not_flagged`) is a leading-INDENT case, not the
/// mid-line CONCATENATION case criterion 7f targets — the spec text is
/// explicit that the trade-off the column-0 anchor makes is against
/// concatenation, not indentation, so the low-confidence pass should not
/// re-flag indentation either.
// trace:TASK-1313 | ai:claude
fn is_indent_anchored(bytes: &[u8], start: usize) -> bool {
    let mut i = start;
    loop {
        if i == 0 {
            return true;
        }
        match bytes[i - 1] {
            b'\n' => return true,
            b' ' | b'\t' => i -= 1,
            _ => return false,
        }
    }
}

/// Scan every text field of every requirement in the store — title,
/// description, and every comment body, walked recursively into nested
/// replies — for round-trip artifacts. Coverage is the whole object store,
/// not just descriptions. Report only: never mutates `store`. All reported
/// offsets are BYTE offsets into the field's UTF-8 text (`str::find`/regex
/// match positions, not character counts), matching what a byte-oriented
/// editor or `sed`/`grep -b` would report.
// trace:TASK-1313 | ai:claude
fn scan_round_trip_artifacts(store: &aida_core::models::RequirementsStore) -> Vec<DoctorFinding> {
    let toon_re = round_trip_toon_header_regex();
    let toon_re_unanchored = round_trip_toon_header_regex_unanchored();
    let mojibake_re = round_trip_mojibake_regex();
    let mut out = Vec::new();

    for req in &store.requirements {
        let spec_id = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());
        let mut fields: Vec<(String, &str)> = vec![
            ("title".to_string(), req.title.as_str()),
            ("description".to_string(), req.description.as_str()),
        ];
        push_comment_fields(&req.comments, "", &mut fields);

        for (field, text) in fields {
            let bytes = text.as_bytes();
            for m in toon_re.find_iter(text) {
                out.push(DoctorFinding {
                    category: "round-trip-artifacts".to_string(),
                    id: format!("{spec_id}/{field}/{}", m.start()),
                    summary: format!(
                        "{spec_id} field `{field}` byte offset {}: swallowed TOON row header (`{}`) — a read-modify-write through rendered `aida show` output",
                        m.start(),
                        m.as_str()
                    ),
                    action: "reconstruct the field from the orphan branch's git history (this check reports, it does not repair)".to_string(),
                    safe_heal: false,
                });
            }
            // Criterion 7f: a second, unanchored pass for a header that was
            // appended MID-LINE — a shape the column-0-anchored primary
            // predicate cannot see. Only reported when NOT also an anchored
            // hit (start of text or immediately after a newline), so this
            // never duplicates the high-confidence findings above; kept in a
            // distinct category/summary ("possible, mid-line") because
            // unanchored also matches ordinary inline prose discussing the
            // convention — noisier by design, eyeballed by a human, not
            // folded into the primary signal.
            for m in toon_re_unanchored.find_iter(text) {
                let start = m.start();
                if is_indent_anchored(bytes, start) {
                    continue;
                }
                out.push(DoctorFinding {
                    category: "round-trip-artifacts-possible".to_string(),
                    id: format!("{spec_id}/{field}/{start}"),
                    summary: format!(
                        "{spec_id} field `{field}` byte offset {start}: possible, mid-line TOON row header (`{}`) — not at the start of a line, so lower confidence than the anchored check; may be a swallowed block appended without a leading newline, or ordinary prose mentioning the convention",
                        m.as_str()
                    ),
                    action: "eyeball the surrounding text; if it is a genuine swallowed block, reconstruct the field from the orphan branch's git history (this check reports, it does not repair)".to_string(),
                    safe_heal: false,
                });
            }
            for m in mojibake_re.find_iter(text) {
                out.push(DoctorFinding {
                    category: "round-trip-artifacts".to_string(),
                    id: format!("{spec_id}/{field}/{}", m.start()),
                    summary: format!(
                        "{spec_id} field `{field}` byte offset {}: UTF-8-decoded-as-Latin-1 mojibake sequence (`{}`)",
                        m.start(),
                        m.as_str()
                    ),
                    action: "reconstruct the field from the orphan branch's git history (this check reports, it does not repair)".to_string(),
                    safe_heal: false,
                });
            }
        }
    }

    out
}

/// Whether `category` (a normalized doctor category) is in scope given the
/// user's `--category` filter. `None` filter selects everything. Errors only
// if the filter itself is an unknown category. trace:TASK-673 | ai:claude
fn doctor_category_selected(filter: Option<&str>, category: &str) -> Result<bool> {
    match filter {
        None => Ok(true),
        Some(raw) => Ok(normalize_doctor_category(raw)? == category),
    }
}

#[cfg(test)]
mod task_1313_round_trip_artifact_tests {
    use super::*;
    use aida_core::models::{Requirement, RequirementsStore};

    fn store_with(description: &str) -> RequirementsStore {
        let mut req = Requirement::new("Sample".to_string(), description.to_string());
        req.spec_id = Some("TASK-9001".to_string());
        let mut store = RequirementsStore::new();
        store.requirements.push(req);
        store
    }

    // 7c: raw-form mis-decode (the parser has already turned the escape into
    // the control character) produces exactly one mojibake finding.
    #[test]
    fn mojibake_raw_form_is_found() {
        let text = "the em-dash reads as \u{00E2}\u{0080}\u{0094} here";
        let store = store_with(text);
        let findings = scan_round_trip_artifacts(&store);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, "round-trip-artifacts");
        assert!(findings[0].summary.contains("mojibake"));
    }

    // 7d: escaped-form mis-decode (the field still carries the literal
    // `\xHH` escape text) also produces one finding.
    #[test]
    fn mojibake_escaped_form_is_found() {
        let text = "the em-dash reads as \u{00E2}\\x80\\x94 here";
        let store = store_with(text);
        let findings = scan_round_trip_artifacts(&store);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].summary.contains("mojibake"));
    }

    // 7a: a legitimate lone Â/Ã/â (an ordinary accented letter, not a pair)
    // produces no mojibake finding.
    #[test]
    fn lone_latin_letter_is_not_mojibake() {
        let text = "The café menu references the \u{00C2}ge Bracket column.";
        let store = store_with(text);
        assert!(scan_round_trip_artifacts(&store).is_empty());
    }

    // 7e: a swallowed TOON block with no non-ASCII character at all is still
    // found — this is the case the mojibake-only detector would have missed.
    #[test]
    fn swallowed_toon_block_without_mojibake_is_found() {
        let text = "Closing the description here.\"\nrelationships[1]{rel,id,title}:\n  child,TASK-2,\"x\"\nnext[1]{cmd,to}:\n  aida queue done TASK-2,done";
        let store = store_with(text);
        let findings = scan_round_trip_artifacts(&store);
        assert_eq!(findings.len(), 2, "expects one finding per anchored header");
        assert!(findings
            .iter()
            .all(|f| f.summary.contains("TOON row header")));
    }

    // 7b (inline mention, negative case 1): prose that discusses the
    // convention mid-line — never at column 0 — must not fire the
    // high-confidence anchored check. This is TASK-1313's own defining
    // case: its spec text contains the phrase `execution_mode:` and the
    // literal pattern `next[1]{` inline, never at the start of a line. An
    // ordinary inline mention like this IS expected to surface in the
    // separately-labelled, lower-confidence mid-line pass (criterion 7f) —
    // that pass is deliberately noisier — so the negative assertion here is
    // scoped to the high-confidence category, not to zero findings overall.
    #[test]
    fn inline_mention_of_execution_mode_is_not_flagged_high_confidence() {
        let text = "It proposes a new execution_mode: value for the advisor to groom.";
        let store = store_with(text);
        let findings = scan_round_trip_artifacts(&store);
        assert!(
            findings
                .iter()
                .all(|f| f.category != "round-trip-artifacts"),
            "an inline mention must never fire the high-confidence anchored check: {findings:?}"
        );
    }

    #[test]
    fn inline_mention_of_toon_header_is_not_flagged_high_confidence() {
        let text = "aida status already uses the `next[1]{cmd,to}:` row header today.";
        let store = store_with(text);
        let findings = scan_round_trip_artifacts(&store);
        assert!(
            findings
                .iter()
                .all(|f| f.category != "round-trip-artifacts"),
            "an inline mention must never fire the high-confidence anchored check: {findings:?}"
        );
    }

    // Criterion 7f: a swallowed TOON row header appended MID-LINE — no
    // newline before it, because the capture path stripped (or never had)
    // the trailing newline — is still found, just at lower confidence and
    // in a separately-labelled category so it does not dilute the
    // high-confidence anchored findings. Both anchored forms (column-0 and
    // indent-allowing) miss this shape by construction.
    #[test]
    fn mid_line_toon_header_is_found_as_possible() {
        let text = "Closing the description here.\"execution_mode: drain";
        let store = store_with(text);
        let findings = scan_round_trip_artifacts(&store);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].category, "round-trip-artifacts-possible");
        assert!(findings[0].summary.contains("possible, mid-line"));
        assert!(findings[0].summary.contains("byte offset"));
    }

    // Follow-up (criterion 7b): TASK-1313's own defining sentence and the
    // raw regex source line, verbatim, as a fixture. The spec argues this is
    // the strongest negative case: the `^execution_mode:` clause of the
    // regex's own SOURCE TEXT matches the pattern mid-line (never at column
    // 0), so a fix that special-cased this spec by id would still have to
    // reckon with the pattern re-appearing in its own defining prose.
    #[test]
    fn task_1313_own_defining_text_is_not_flagged_high_confidence() {
        let text = "THE PRIMARY PREDICATE IS THE CAUSE, NOT THE SYMPTOM, AND IT IS LINE-ANCHORED AT COLUMN 0. A description containing a TOON row header AT THE START OF A LINE is a round-trip artifact:\n\n       ^(?:relationships|next)\\[\\d+\\]\\{      or      ^execution_mode:\n";
        let store = store_with(text);
        let findings = scan_round_trip_artifacts(&store);
        assert!(
            findings.iter().all(|f| f.category != "round-trip-artifacts"),
            "TASK-1313's own defining text must never fire the high-confidence anchored check: {findings:?}"
        );
    }

    // 7b (negative case 2): an INDENTED code block quoting a TOON sample —
    // the column-0 anchor (not a leading-indent-allowing anchor) must not
    // match an indented line.
    #[test]
    fn indented_toon_sample_is_not_flagged() {
        let text = "See the example below:\n\n    relationships[1]{rel,id,title}:\n      child,TASK-2,\"x\"\n";
        let store = store_with(text);
        assert!(scan_round_trip_artifacts(&store).is_empty());
    }

    // Coverage: title and comment bodies are scanned too, not only
    // description (criterion 6).
    #[test]
    fn title_and_comment_fields_are_scanned() {
        let mut req = Requirement::new(
            "execution_mode:\nswallowed into the title field".to_string(),
            "clean description".to_string(),
        );
        req.spec_id = Some("TASK-9002".to_string());
        req.comments.push(aida_core::models::Comment::new(
            "joe".to_string(),
            "comment carries \u{00C3}\u{00A2} mojibake".to_string(),
        ));
        let mut store = RequirementsStore::new();
        store.requirements.push(req);
        let findings = scan_round_trip_artifacts(&store);
        assert!(findings.iter().any(|f| f.id.contains("/title/")));
        assert!(findings.iter().any(|f| f.id.contains("/comment[0]/")));
    }

    // Criterion 6: coverage is every text field, including a NESTED reply
    // (`Comment.replies: Vec<Comment>`), not just top-level comment bodies.
    // Mojibake buried two levels deep (a reply's reply) must still surface.
    #[test]
    fn mojibake_in_nested_reply_is_found() {
        let mut req = Requirement::new("Sample".to_string(), "clean description".to_string());
        req.spec_id = Some("TASK-9003".to_string());
        let top = aida_core::models::Comment::new(
            "joe".to_string(),
            "clean top-level comment".to_string(),
        );
        let top_id = top.id;
        let mut reply = aida_core::models::Comment::new_reply(
            "advisor".to_string(),
            "clean first-level reply".to_string(),
            top_id,
        );
        let reply_id = reply.id;
        let nested_reply = aida_core::models::Comment::new_reply(
            "advisor".to_string(),
            "the em-dash reads as \u{00E2}\u{0080}\u{0094} here, two levels deep".to_string(),
            reply_id,
        );
        reply.replies.push(nested_reply);
        let mut top = top;
        top.replies.push(reply);
        req.comments.push(top);

        let mut store = RequirementsStore::new();
        store.requirements.push(req);
        let findings = scan_round_trip_artifacts(&store);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert!(findings[0].summary.contains("mojibake"));
        assert!(
            findings[0].id.contains("/comment[0]/reply[0]/reply[0]/"),
            "expected a doubly-nested reply field label, got {}",
            findings[0].id
        );
    }
}

#[cfg(test)]
mod story_762_vendor_binary_tests {
    use super::*;

    // STORY-762: the codex-only doctor category resolves through the
    // normalizer with its aliases; which_binary finds a real binary and
    // returns None for a nonsense name.
    #[test]
    fn vendor_binary_category_normalizes_with_aliases() {
        for raw in ["vendor-binary", "vendor-binaries", "vendor", "VENDORS"] {
            assert_eq!(normalize_doctor_category(raw).unwrap(), "vendor-binary");
        }
    }

    #[test]
    fn which_binary_finds_sh_and_misses_nonsense() {
        // `sh` exists on every unix CI runner; a random token does not.
        #[cfg(unix)]
        assert!(which_binary("sh").is_some());
        assert!(which_binary("definitely-not-a-real-binary-xyzzy").is_none());
    }
}

/// TASK-1089: the git-canonical storage migration cutoff. Completions BEFORE
/// this date predate the reliable `(SPEC-ID)` trailer + trace convention, and
/// the bulk YAML import stamped every pre-migration spec's `created_at` at
/// import time — so flagging them as "completed without commit" is pure noise
/// (the observed 353→212 import-cohort false positives). Completions on/after it
/// are expected to carry git corroboration. A FIXED migration date, not a
/// rolling window: a window would slide forward and eventually hide genuinely
/// stranded recent completions. Override per-run with `--since` /
/// `AIDA_DOCTOR_COMPLETED_SINCE`. Pinned to UTC midnight so the count does
/// not depend on the machine's timezone.
// trace:TASK-1089 | ai:claude
// trace:TASK-1509 | ai:claude
const GIT_CANONICAL_MIGRATION_CUTOFF: &str = "2026-06-01T00:00:00Z";

fn default_completed_without_commit_recent_cutoff() -> Option<String> {
    Some(GIT_CANONICAL_MIGRATION_CUTOFF.to_string())
}

// ============================================================================
// TASK-717 — `aida doctor`: verify-and-prune stale REMOTE branches.
//
// `aida doctor --heal --category orphan-branches` only deletes LOCAL orphan
// branches; stale `origin/*` branches accumulate and must be pruned by hand.
// This adds a `stale-remote-branches` category that surfaces remote branches
// and verify-and-prunes them with a squash-aware safety model:
//
//   safe-to-delete iff ANY of:
//     - the branch's spec is Completed/Rejected, OR
//     - the branch HEAD is an ancestor of origin/main (already merged), OR
//     - origin/main carries a commit referencing the spec `(SPEC-ID)`
//   AND none of the EXCLUDE conditions apply:
//     - the branch is a protected ref (main/master/aida-store), OR
//     - the branch has an open PR
//   AND the branch has NO genuinely-unique unmerged commits (those are KEPT
//   and flagged for the operator, never deleted).
//
// Dry-run by default (read-only list + per-branch verdict/reason); the heal
// is gated behind --yes --force (destructive remote deletion). trace:TASK-717
// ============================================================================

// The classification verdict for one remote branch. trace:TASK-717
#[derive(Debug, Clone, PartialEq, Eq)]
enum RemoteBranchVerdict {
    /// Protected or open-PR — excluded from any consideration.
    Excluded(String),
    /// Verified merged/terminal AND no unique unmerged commits → safe to prune.
    SafeToDelete(String),
    /// Has genuinely-unique unmerged commits → keep, flag for the operator.
    Keep(String),
}

/// Inputs to the pure remote-branch classifier — every git/gh probe result the
/// safety model needs, gathered once per branch by the scanner. Keeping the
/// classification pure makes the squash-aware safety model unit-testable
// without git or gh. trace:TASK-717
#[derive(Debug, Clone)]
struct RemoteBranchFacts {
    /// True when the branch is a protected ref (main/master/aida-store).
    protected: bool,
    /// True when an open PR currently has this branch as its head.
    has_open_pr: bool,
    /// True when the branch HEAD is an ancestor of origin/main (fully merged —
    /// covers fast-forward / non-squash merges).
    ancestor_of_main: bool,
    /// True when origin/main carries a commit whose subject/trailer references
    /// the branch's spec id (covers squash merges, where the branch tip keeps a
    /// different SHA but the work shipped).
    spec_referenced_on_main: bool,
    /// The branch's spec status, when the branch name maps to a known spec and
    /// that spec is Completed or Rejected (terminal). None otherwise.
    spec_terminal: bool,
    /// Count of commits on the branch not reachable from origin/main. Zero means
    /// nothing unique is at risk; non-zero with no other "merged" signal means
    /// the branch carries unique unmerged work that must be KEPT.
    unique_unmerged_commits: u32,
}

/// Pure squash-aware classification of one remote branch. No git/gh — every
/// input is pre-gathered in `RemoteBranchFacts`. This is the safety model:
/// excludes protected + open-PR branches first; declares safe-to-delete only on
/// a positive "merged/terminal" signal AND zero unique unmerged commits;
// otherwise keeps the branch and flags it. trace:TASK-717
fn classify_stale_remote_branch(facts: &RemoteBranchFacts) -> RemoteBranchVerdict {
    // EXCLUDE first — protected and open-PR branches are never candidates.
    if facts.protected {
        return RemoteBranchVerdict::Excluded("protected ref".to_string());
    }
    if facts.has_open_pr {
        return RemoteBranchVerdict::Excluded("has an open PR".to_string());
    }

    // A positive "this work has shipped / is terminal" signal — any one suffices.
    let merged_reason = if facts.ancestor_of_main {
        Some("HEAD is an ancestor of origin/main (merged)")
    } else if facts.spec_referenced_on_main {
        Some("origin/main carries a commit referencing its spec (squash-merged)")
    } else if facts.spec_terminal {
        Some("its spec is Completed/Rejected")
    } else {
        None
    };

    let Some(merged_reason) = merged_reason else {
        // No merged/terminal signal at all → never delete; flag for the operator.
        return RemoteBranchVerdict::Keep(
            "no merge/terminal signal — operator decision".to_string(),
        );
    };

    // Even with a merged signal, a branch carrying genuinely-unique unmerged
    // commits (e.g. an unmerged migration-guide on a squash-merged branch) must
    // be KEPT — deleting it would lose work. Ancestor-of-main implies zero unique
    // commits, so this only bites the squash/terminal paths.
    if facts.unique_unmerged_commits > 0 && !facts.ancestor_of_main {
        return RemoteBranchVerdict::Keep(format!(
            "{merged_reason}, but {} unique unmerged commit(s) — keep, operator decision",
            facts.unique_unmerged_commits
        ));
    }

    RemoteBranchVerdict::SafeToDelete(merged_reason.to_string())
}

/// Derive the candidate spec id from a work-branch name (`task-281-foo` →
/// `TASK-281`). Returns None when the branch doesn't follow the work-branch
// convention. trace:TASK-717
// trace:BUG-888 | ai:codex
fn spec_id_from_work_branch(branch: &str) -> Option<String> {
    crate::work_spec_id_from_branch(branch)
}

/// Do the shared branches (code trunk + orphan store) hold the same tip on
/// every configured remote? A mismatch means the substrate has forked across
/// hubs (e.g. a clone pushed the store to gitlab but not github). Uses cheap
/// `ls-remote` (refs only, no object transfer) for current truth; a remote we
/// can't reach is skipped (not a false alarm). Never writes. Emits one finding
/// per diverged branch.
// trace:TASK-1095 | ai:claude
/// STORY-781: check the checked-in project manifest.
///
/// ABSENCE PRODUCES NO FINDING, deliberately and load-bearingly. A project
/// without a manifest keeps working identically; treating its absence as a
/// defect would make every pre-existing repository suddenly "unhealthy" for a
/// file it never agreed to carry, and would turn an optional standard into a
/// nag. Only things a human can act on are reported:
///
///   - malformed  — the file exists but cannot be read
///   - unfilled   — scaffolded and never completed; the blank form this whole
///                  design exists to prevent
///   - drifted    — the recorded repository no longer matches `origin`
///
/// Read-only. Never mutates, never heals.
// trace:STORY-781 | ai:claude
pub(crate) fn scan_project_manifest(
    project_root: &std::path::Path,
    store: &aida_core::RequirementsStore,
) -> Vec<DoctorFinding> {
    use aida_core::project_manifest as pm;

    let mut findings = Vec::new();
    let manifest = match pm::load(project_root) {
        // Not a defect. Say nothing.
        pm::ManifestState::Absent => return findings,
        pm::ManifestState::Malformed(msg) => {
            findings.push(DoctorFinding {
                category: "project-manifest".to_string(),
                id: "project-manifest-malformed".to_string(),
                summary: format!("{} could not be read: {msg}", pm::MANIFEST_REL_PATH),
                action: format!(
                    "fix the TOML in {} (schema: docs/project-manifest.md).                      Nothing else is affected — every command keeps working without it",
                    pm::MANIFEST_REL_PATH
                ),
                safe_heal: false,
            });
            return findings;
        }
        pm::ManifestState::Present(m) => m,
    };

    let real_enough_for_identity_nudge =
        project_identity_is_real_enough(project_root, store, &manifest);

    if manifest.is_unfilled() && real_enough_for_identity_nudge {
        findings.push(DoctorFinding {
            category: "project-manifest".to_string(),
            id: "project-manifest-unfilled".to_string(),
            summary: format!(
                "{} was scaffolded but never filled in — nothing is recorded that a scan could not already work out",
                pm::MANIFEST_REL_PATH
            ),
            action: "add at least `why` — the one thing no tool can derive for you".to_string(),
            safe_heal: false,
        });
    }

    // trace:STORY-789 | ai:codex — the thesis lives as a VISION, not as a
    // duplicate manifest field, and doctor only nudges once the project has
    // earned that ask.
    if real_enough_for_identity_nudge && !store.requirements.iter().any(is_project_vision) {
        findings.push(DoctorFinding {
            category: "project-manifest".to_string(),
            id: "project-thesis-missing".to_string(),
            summary: "no project thesis VISION is recorded — the governing bet is still implicit"
                .to_string(),
            action: "create one with `aida add --type vision --title \"Project thesis\"` and phrase it as the bet this project is making".to_string(),
            safe_heal: false,
        });
    }

    // Values this build does not recognise. The FILE still parsed — that is the
    // degrade-not-reject contract — but the author almost certainly believes
    // they said something meaningful, so say that we did not understand it.
    // Note this fires on a genuinely-newer schema too, which is the right
    // trade: "your AIDA is older than this manifest" is worth knowing.
    let mut unknown: Vec<String> = Vec::new();
    if let Some(l) = manifest.project.liveness.as_ref() {
        if !l.is_recognised() {
            unknown.push(format!(
                "liveness = \"{}\" (expected one of: {})",
                l.as_str(),
                pm::Liveness::ALL.join(", ")
            ));
        }
    }
    if let Some(st) = manifest.project.stage.as_ref() {
        if !st.is_recognised() {
            unknown.push(format!(
                "stage = \"{}\" (expected one of: {})",
                st.as_str(),
                pm::Stage::ALL.join(", ")
            ));
        }
    }
    if !unknown.is_empty() {
        findings.push(DoctorFinding {
            category: "project-manifest".to_string(),
            id: "project-manifest-unrecognised-value".to_string(),
            summary: format!(
                "{} uses a value this build does not recognise: {}",
                pm::MANIFEST_REL_PATH,
                unknown.join("; ")
            ),
            action: format!(
                "fix the value, or upgrade `aida` if this manifest was written by a newer one.                  The rest of {} is still being read normally",
                pm::MANIFEST_REL_PATH
            ),
            safe_heal: false,
        });
    }

    // A recorded repository that no longer matches `origin` means the project
    // moved and the manifest is now describing somewhere it does not live.
    if let Some(recorded) = manifest.project.repository.as_deref() {
        if let Some(actual) = aida_core::git_ops::remote_url(project_root, "origin") {
            if !same_remote(recorded, &actual) {
                findings.push(DoctorFinding {
                    category: "project-manifest".to_string(),
                    id: "project-manifest-repository-drift".to_string(),
                    summary: format!(
                        "{} records repository `{recorded}` but origin is `{actual}`",
                        pm::MANIFEST_REL_PATH
                    ),
                    action: format!(
                        "update `repository` in {} to match, or remove it",
                        pm::MANIFEST_REL_PATH
                    ),
                    safe_heal: false,
                });
            }
        }
    }

    findings
}

fn is_project_vision(req: &aida_core::Requirement) -> bool {
    req.req_type == aida_core::RequirementType::Vision
}

fn project_identity_is_real_enough(
    project_root: &std::path::Path,
    store: &aida_core::RequirementsStore,
    manifest: &aida_core::project_manifest::ProjectManifest,
) -> bool {
    use aida_core::project_manifest::Liveness;

    if matches!(
        manifest.project.liveness.as_ref(),
        Some(Liveness::Parked | Liveness::Abandoned)
    ) {
        return false;
    }

    if store.requirements.iter().any(|r| {
        r.req_type != aida_core::RequirementType::Meta
            && !r.tags.contains("from-aida-init")
            && !r.title.trim().is_empty()
    }) {
        return true;
    }

    git_commit_count(project_root) >= 3
}

fn git_commit_count(project_root: &std::path::Path) -> usize {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-list", "--count", "HEAD"])
        .output();
    let Ok(out) = out else {
        return 0;
    };
    if !out.status.success() {
        return 0;
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

/// Whether two clone URLs name the same repository.
///
/// The same repo is written `https://host/o/n.git`, `git@host:o/n.git` and
/// `ssh://git@host/o/n` — comparing raw strings would report drift on every
/// project whose manifest was written from one form and whose remote uses
/// another. Reduced to `host/owner/name`.
// trace:STORY-781 | ai:claude
pub(crate) fn same_remote(a: &str, b: &str) -> bool {
    fn key(url: &str) -> String {
        let u = url.trim().trim_end_matches('/');
        let u = u.strip_suffix(".git").unwrap_or(u);
        // scp-style: git@host:owner/name
        if let Some(rest) = u.split_once('@').map(|(_, r)| r) {
            if let Some((host, path)) = rest.split_once(':') {
                if !path.starts_with("//") {
                    return format!("{}/{}", host.to_lowercase(), path.trim_start_matches('/'));
                }
            }
        }
        let after_scheme = u.split_once("://").map(|(_, r)| r).unwrap_or(u);
        let after_user = after_scheme
            .split_once('@')
            .map(|(_, r)| r)
            .unwrap_or(after_scheme);
        match after_user.split_once('/') {
            Some((host, path)) => format!("{}/{}", host.to_lowercase(), path),
            None => after_user.to_lowercase(),
        }
    }
    key(a) == key(b)
}

fn scan_remote_drift(project_root: &std::path::Path) -> Vec<DoctorFinding> {
    let remotes: Vec<String> = aida_core::git_ops::list_remotes(project_root)
        .into_iter()
        .filter(|r| r != "all")
        .collect();
    if remotes.len() < 2 {
        return Vec::new();
    }

    let mut findings = Vec::new();
    for branch in ["main", "aida-store"] {
        // (remote, short-sha) for every remote that currently has the branch.
        let tips: Vec<(String, String)> = remotes
            .iter()
            .filter_map(|r| {
                aida_core::git_ops::remote_branch_head_sha(project_root, r, branch)
                    .map(|sha| (r.clone(), sha.chars().take(12).collect::<String>()))
            })
            .collect();
        if tips.len() < 2 {
            continue; // can't compare (offline, or only one hub has it)
        }
        let distinct: std::collections::BTreeSet<&String> = tips.iter().map(|(_, s)| s).collect();
        if distinct.len() > 1 {
            let detail = tips
                .iter()
                .map(|(r, s)| format!("{r}={s}"))
                .collect::<Vec<_>>()
                .join(" ");
            findings.push(DoctorFinding {
                category: "remote-drift".to_string(),
                id: format!("remote-drift-{branch}"),
                summary: format!("branch `{branch}` differs across remotes: {detail}"),
                action: if branch == "aida-store" {
                    "run `aida remote reconcile` (dry-run; --execute to union-merge and push every hub); \
                     never force-push a shared branch to resolve"
                        .to_string()
                } else {
                    "reconcile the divergent tips and push to every remote (see `aida remote status`); \
                     never force-push a shared branch to resolve"
                        .to_string()
                },
                safe_heal: false,
            });
        }
    }
    // trace:STORY-1048 | ai:codex
    if let Some(tag) = crate::remote_create::latest_local_release_tag(project_root) {
        let standings =
            crate::remote_create::collect_release_standings(project_root, &remotes, &tag);
        let missing_tags: Vec<String> = standings
            .iter()
            .filter(|s| s.tag_head.is_none())
            .map(|s| s.remote.clone())
            .collect();
        if !missing_tags.is_empty() {
            findings.push(DoctorFinding {
                category: "remote-drift".to_string(),
                id: format!("remote-drift-release-tag-{tag}"),
                summary: format!(
                    "release tag `{tag}` is missing on remote(s): {}",
                    missing_tags.join(", ")
                ),
                action: format!(
                    "push the release tag to every hub: {}",
                    missing_tags
                        .iter()
                        .map(|r| format!("`git push {r} {tag}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                safe_heal: false,
            });
        }

        let missing_releases: Vec<String> = standings
            .iter()
            .filter(|s| matches!(s.gitlab_release_exists, Some(false)))
            .map(|s| s.remote.clone())
            .collect();
        if !missing_releases.is_empty() {
            findings.push(DoctorFinding {
                category: "remote-drift".to_string(),
                id: format!("remote-drift-gitlab-release-{tag}"),
                summary: format!(
                    "GitLab release `{tag}` is missing on remote(s): {}",
                    missing_releases.join(", ")
                ),
                action: "rerun the mirrored tag pipeline or create the GitLab release with the four uploaded assets".to_string(),
                safe_heal: false,
            });
        }
    }
    findings
}

/// TASK-717: scan stale `origin/*` branches and classify each under the
/// squash-aware safety model. Read-only — performs git ancestry/rev-list probes
/// and one `gh pr list` for the open-PR set, never mutates anything. Returns a
/// `stale-remote-branches` finding for every branch that is either SafeToDelete
/// (verdict carries the merge reason; `safe_heal=false` so deletion stays gated
/// behind --yes --force) or Keep (flagged for the operator, never auto-healed).
// Excluded branches produce no finding. trace:TASK-717
fn scan_stale_remote_branches(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
) -> Vec<DoctorFinding> {
    use std::process::Command as PCmd;

    let git = |args: &[&str]| -> Option<String> {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };

    // No resolvable default branch → we cannot corroborate merges, so stay
    // silent rather than risk flagging every remote branch.
    let Some(default_ref) = resolve_default_branch_ref(project_root) else {
        return Vec::new();
    };

    // Spec ids referenced by commits on origin/main (squash-merge signal).
    let mut referenced_on_main: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    if let Some(log) = git(&["log", "--pretty=format:%s", &default_ref]) {
        for subject in log.lines() {
            let subject = subject.trim();
            if subject.is_empty() || is_plan_commit_subject(subject) {
                continue;
            }
            for id in extract_spec_ids_from_commit(subject) {
                referenced_on_main.insert(id.to_ascii_uppercase());
            }
        }
    }

    let open_pr_branches = collect_open_prs(project_root).by_branch;
    const PROTECTED: &[&str] = &["main", "master", "aida-store", "HEAD"];

    let mut findings = Vec::new();
    for rc in collect_remote_branch_commits(project_root) {
        let branch = rc.branch;
        let protected = PROTECTED.iter().any(|p| p.eq_ignore_ascii_case(&branch));
        let has_open_pr = open_pr_branches.contains_key(&branch);

        // Ancestor-of-main check: empty `origin/main..origin/branch` rev-list.
        let ancestor_of_main = git(&[
            "rev-list",
            "--count",
            &format!("{default_ref}..origin/{branch}"),
        ])
        .and_then(|s| s.trim().parse::<u32>().ok())
        .map(|n| n == 0)
        .unwrap_or(false);

        // unique_unmerged_commits == ancestor-count above; reuse it.
        let unique_unmerged_commits = git(&[
            "rev-list",
            "--count",
            &format!("{default_ref}..origin/{branch}"),
        ])
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);

        let spec_id = spec_id_from_work_branch(&branch);
        let spec_referenced_on_main = spec_id
            .as_deref()
            .map(|id| referenced_on_main.contains(&id.to_ascii_uppercase()))
            .unwrap_or(false);
        let spec_terminal = spec_id
            .as_deref()
            .and_then(|id| store.get_requirement_by_spec_id(id))
            .map(|req| {
                matches!(
                    req.status,
                    RequirementStatus::Completed | RequirementStatus::Rejected
                )
            })
            .unwrap_or(false);

        let facts = RemoteBranchFacts {
            protected,
            has_open_pr,
            ancestor_of_main,
            spec_referenced_on_main,
            spec_terminal,
            unique_unmerged_commits,
        };

        match classify_stale_remote_branch(&facts) {
            RemoteBranchVerdict::Excluded(_) => {
                // Protected / open-PR — never surfaced as a finding.
            }
            RemoteBranchVerdict::SafeToDelete(reason) => {
                findings.push(DoctorFinding {
                    category: "stale-remote-branches".to_string(),
                    id: branch.clone(),
                    summary: format!("remote branch `origin/{branch}` is stale ({reason})"),
                    action: format!(
                        "delete remote branch (`aida doctor --heal --category \
                         stale-remote-branches --yes --force`, or `git push origin \
                         --delete {branch}`)"
                    ),
                    // Remote deletion is destructive — gate behind --yes --force.
                    safe_heal: false,
                });
            }
            RemoteBranchVerdict::Keep(reason) => {
                findings.push(DoctorFinding {
                    category: "stale-remote-branches".to_string(),
                    id: branch.clone(),
                    summary: format!("remote branch `origin/{branch}` flagged: {reason}"),
                    action: "operator decision: review and keep, open a PR, or delete by hand"
                        .to_string(),
                    // Never auto-deleted — flag-only.
                    safe_heal: false,
                });
            }
        }
    }
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    findings
}

/// TASK-717: delete one stale remote branch via `git push origin --delete`.
/// Only reachable when the finding's action surfaced it as SafeToDelete and the
/// caller passed --yes --force (see `heal_doctor_finding`). A Keep-flagged
// branch never routes here — its action string is operator-only. trace:TASK-717
fn heal_doctor_stale_remote_branch(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    // Keep-flagged findings carry an operator-only action and must never be
    // auto-deleted, even under --yes --force.
    if !finding.action.contains("--category stale-remote-branches") {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some("flagged for operator decision — not auto-deleted".to_string()),
        });
    }
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        // The id is a remote branch name; end options before it.
        // trace:BUG-1622 | ai:claude
        .args([
            "push",
            "--delete",
            git_arg_guard::END_OF_OPTIONS,
            "origin",
            &finding.id,
        ])
        .status()
        .with_context(|| format!("deleting remote branch origin/{}", finding.id))?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "deleted stale remote branch".to_string(),
        status: if status.success() { "healed" } else { "failed" }.to_string(),
        detail: None,
    })
}

// The classification verdict for one agent-managed worktree. trace:TASK-878
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentWorktreeVerdict {
    /// Verified merged AND clean AND no unique unmerged commits → safe to GC.
    Removable(String),
    /// Dirty, carrying unmerged work, or no merge signal → keep, flag operator.
    Keep(String),
}

/// Inputs to the pure agent-worktree classifier — every git/forge probe result
/// the safety model needs, gathered once per worktree by the scanner. Keeping
/// the classification pure makes the squash-aware safety model unit-testable
// without git or a forge. trace:TASK-878
#[derive(Debug, Clone)]
pub(crate) struct AgentWorktreeFacts {
    /// True when the worktree has uncommitted changes (tracked or staged dirt,
    /// or untracked files). A dirty worktree is NEVER removed — clean != no work,
    /// and dirty is unambiguously "has work".
    pub(crate) dirty: bool,
    /// True when the branch HEAD is an ancestor of origin/main (fully merged —
    /// covers fast-forward / non-squash merges; implies zero unique commits).
    pub(crate) ancestor_of_main: bool,
    /// True when the forge reports a MERGED PR for this branch (covers squash
    /// merges, where the branch tip keeps a different SHA but the work shipped).
    pub(crate) pr_merged: bool,
    /// Count of commits on the branch not reachable from origin/main. Zero means
    /// nothing unique is at risk; non-zero with no ancestor signal means the
    /// branch carries unique unmerged work that must be KEPT — UNLESS
    /// `content_fully_landed` clears it (see below).
    pub(crate) unique_unmerged_commits: u32,
    /// True when a CONTENT comparison (not ancestry) proves every file the
    /// branch's own commits touched already matches origin/main's current
    /// tree byte-for-byte. A squash merge gives the branch's original commits
    /// a permanently different SHA from the squash commit on main, so plain
    /// ancestry — and a naive commit count derived from it — can NEVER clear
    /// a squash-merged branch; `unique_unmerged_commits` stays positive
    /// forever even when nothing unique remains (BUG-1287). This field is the
    /// squash-aware refinement: computed only when `pr_merged` is true, it
    /// lets a confirmed-merged branch with a positive commit count still be
    /// classified Removable when content proves the count is an ancestry
    /// artifact, while a branch that genuinely carries post-merge work (extra
    /// commits whose content is NOT yet reflected on main) still Keeps.
    /// Never set true when there is any doubt — the caller (`scan_*`) leaves
    /// it false whenever the git probes needed to prove it are inconclusive.
    // trace:BUG-1287 | ai:claude
    pub(crate) content_fully_landed: bool,
}

/// Pure squash-aware classification of one agent-managed worktree. No git/forge
/// — every input is pre-gathered in `AgentWorktreeFacts`. This is the safety
/// model: a dirty worktree is always kept; removal needs a positive merged
/// signal AND zero unique unmerged commits; otherwise the worktree is kept and
// flagged. trace:TASK-878 | ai:claude
pub(crate) fn classify_agent_worktree(facts: &AgentWorktreeFacts) -> AgentWorktreeVerdict {
    // KEEP first — uncommitted work is unambiguously "has work". Clean != no
    // work, but dirty is definitely work; never delete it.
    if facts.dirty {
        return AgentWorktreeVerdict::Keep(
            "uncommitted changes present — never auto-removed".to_string(),
        );
    }

    // A positive "this work has shipped" signal — either suffices.
    let merged_reason = if facts.ancestor_of_main {
        Some("branch is an ancestor of origin/main (merged)")
    } else if facts.pr_merged {
        Some("its PR is merged (squash-merged)")
    } else {
        None
    };

    let Some(merged_reason) = merged_reason else {
        // No merged signal at all → never remove; flag for the operator.
        return AgentWorktreeVerdict::Keep("no merge signal — operator decision".to_string());
    };

    // Even with a merged signal, a branch carrying genuinely-unique unmerged
    // commits (e.g. extra commits added after the PR squash-merged) must be KEPT
    // — removing it would lose work. Ancestor-of-main implies zero unique
    // commits, so this only bites the squash-merge path.
    if facts.unique_unmerged_commits > 0 && !facts.ancestor_of_main {
        // BUG-1287: a positive commit count on the squash-merge path is not
        // proof of unique WORK — it is guaranteed positive forever by ancestry
        // alone, squash or no squash. `content_fully_landed` is the content-level
        // check that clears the ancestry artifact: every file the branch's own
        // commits touched already matches main byte-for-byte, so nothing is at
        // risk despite the non-zero count.
        if facts.content_fully_landed {
            return AgentWorktreeVerdict::Removable(format!(
                "{merged_reason}, {} commit(s) content-verified fully landed on origin/main \
                 (ancestry alone can't clear a squash-merged branch)",
                facts.unique_unmerged_commits
            ));
        }
        return AgentWorktreeVerdict::Keep(format!(
            "{merged_reason}, but {} unique unmerged commit(s) — keep, operator decision",
            facts.unique_unmerged_commits
        ));
    }

    AgentWorktreeVerdict::Removable(merged_reason.to_string())
}

/// Content-level (not ancestry) proof that `branch`'s own commits are already
/// represented by patch-equivalent commits in `default_ref`. This accepts both
/// per-commit equivalence and a branch's cumulative patch matching one squash
/// commit, because a multi-commit branch has no individual patch equivalent to
/// that combined commit. Patch equivalence is durable: later default-branch
/// commits may change the same files without making already-landed work appear
/// unique again. Every comparison is bounded to commits since the merge-base.
///
/// Conservative on any doubt: an unresolvable merge-base or a failed git call
/// returns `false` (stay KEPT), never `true`. Read-only — no writes, no
/// network.
// trace:BUG-1287 | ai:claude
// trace:BUG-1424 | ai:codex
pub(crate) fn branch_content_fully_landed(
    project_root: &std::path::Path,
    default_ref: &str,
    branch: &str,
) -> bool {
    use std::io::Write;
    use std::process::{Command as PCmd, Stdio};

    let run = |args: &[&str]| -> Option<std::process::Output> {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()
    };
    let patch_id = |patch: &[u8]| -> Option<Option<String>> {
        let mut child = PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["patch-id", "--stable"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        child.stdin.take()?.write_all(patch).ok()?;
        let output = child.wait_with_output().ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8(output.stdout).ok()?;
        let mut lines = stdout.lines().filter(|line| !line.trim().is_empty());
        let Some(line) = lines.next() else {
            return Some(None);
        };
        let id = line.split_whitespace().next()?.to_string();
        if lines.next().is_some() {
            return None;
        }
        Some(Some(id))
    };

    // trace:BUG-1622 | ai:claude
    let Some(mb_out) = run(&[
        "merge-base",
        git_arg_guard::END_OF_OPTIONS,
        default_ref,
        branch,
    ]) else {
        return false;
    };
    if !mb_out.status.success() {
        return false;
    }
    let merge_base = String::from_utf8_lossy(&mb_out.stdout).trim().to_string();
    if merge_base.is_empty() {
        return false;
    }

    // List branch-side commits with no patch-equivalent commit on the default
    // side. `^merge_base` explicitly bounds both histories to the divergent
    // range. Empty output means all branch work landed, even if later commits
    // changed the same paths. Any git failure stays conservative (KEPT).
    let range = format!("{default_ref}...{branch}");
    let Some(unique_out) = run(&[
        "rev-list",
        "--cherry-pick",
        "--right-only",
        &range,
        &format!("^{merge_base}"),
    ]) else {
        return false;
    };
    if !unique_out.status.success() {
        return false;
    }
    if String::from_utf8_lossy(&unique_out.stdout)
        .trim()
        .is_empty()
    {
        return true;
    }

    // A forge commonly combines every branch commit into one squash commit.
    // `rev-list --cherry-pick` cannot recognize that N-to-1 equivalence, so
    // compare the branch's combined diff with each bounded default-side commit.
    // trace:BUG-1622 | ai:claude
    let Some(branch_diff) = run(&[
        "diff",
        "--binary",
        git_arg_guard::END_OF_OPTIONS,
        &merge_base,
        branch,
        "--",
    ]) else {
        return false;
    };
    if !branch_diff.status.success() {
        return false;
    }
    let Some(Some(branch_patch_id)) = patch_id(&branch_diff.stdout) else {
        return false;
    };

    let default_range = format!("{merge_base}..{default_ref}");

    // BUG-1288: this fallback used to spawn a `git show | git patch-id` PAIR
    // per default-side commit (`branch_unshipped_patch_count_default`'s
    // sibling cost). On a long-lived repo an old branch's merge-base can sit
    // thousands of commits behind the default branch, so that per-commit
    // fan-out was the dominant cost of both `aida awaiting --json` and `aida
    // status --full` (measured: one 2,496-commit range took 3m41s of wall
    // clock for THIS SINGLE BRANCH's landed-check, serialized N times across
    // every candidate branch in `collect_unshipped_work_items`). That is "the
    // probe's setup" the BUG-1288 review flagged — not the candidate set,
    // which stays exactly as wide as PR #1999 left it.
    //
    // Two changes, kept independent so each is auditable on its own:
    //
    // 1. Bound the walk. A default-side range wider than
    //    `MAX_SQUASH_FALLBACK_COMMITS` is too expensive to exhaust patch-id
    //    matching over, so it is skipped rather than paid for on every read.
    //    The conservative branch is `false` ("not confirmed landed") — the
    //    branch STAYS in the unshipped-work report rather than being
    //    silently hidden on unproven equivalence (PRIN-5); at worst a
    //    genuinely-landed old branch is reported once more than necessary,
    //    never the reverse.
    // 2. When under the bound, replace the N subprocess PAIRS with exactly
    //    two processes total: one `git log -p` streaming every default-side
    //    commit's diff (each preceded by its full hash, from `--format=%H`),
    //    piped into one `git patch-id --stable`, which associates each
    //    computed id with the commit-hash line that precedes it. This is the
    //    same diff text `git show --format= --binary <commit>` produced per
    //    commit — including the same "no diff" empty patch for a merge
    //    commit `git log -p` doesn't expand by default — so the match result
    //    is unchanged; only the process count drops from O(range) to O(1).
    // trace:BUG-1288 | ai:claude
    const MAX_SQUASH_FALLBACK_COMMITS: usize = 500;
    let Some(count_out) = run(&["rev-list", "--count", &default_range]) else {
        return false;
    };
    if !count_out.status.success() {
        return false;
    }
    let Ok(commit_count) = String::from_utf8_lossy(&count_out.stdout)
        .trim()
        .parse::<usize>()
    else {
        return false;
    };
    if commit_count == 0 {
        return false;
    }
    if commit_count > MAX_SQUASH_FALLBACK_COMMITS {
        return false;
    }

    let Some(log_out) = run(&["log", "--format=%H", "-p", "--binary", &default_range]) else {
        return false;
    };
    if !log_out.status.success() {
        return false;
    }
    let Some(default_side_ids) = patch_id_pairs(project_root, &log_out.stdout) else {
        return false;
    };
    default_side_ids.iter().any(|id| id == &branch_patch_id)
}

/// BUG-1288: batched sibling of the per-commit `patch_id` closure in
/// [`branch_content_fully_landed`] — feeds a whole `git log -p` stream (one
/// commit hash line followed by that commit's diff, repeated) through a
/// SINGLE `git patch-id --stable` process and returns every resulting patch
/// id, instead of spawning one `git patch-id` per commit. `git patch-id`
/// associates each id with the commit-hash line it saw immediately before
/// that diff, so the id/commit pairing this repository doesn't currently need
/// (only the id set is consulted) still falls out of the same single pass.
// trace:BUG-1288 | ai:claude
fn patch_id_pairs(project_root: &std::path::Path, log_p_output: &[u8]) -> Option<Vec<String>> {
    use std::io::Write;
    use std::process::{Command as PCmd, Stdio};

    let mut child = PCmd::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["patch-id", "--stable"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let input = log_p_output.to_vec();
    // BUG-1288 fix-up: writing the WHOLE stream to stdin before reading any
    // stdout deadlocks once the log is large enough to fill both the stdin
    // and stdout OS pipe buffers at once (patch-id blocks writing output
    // because we haven't read it yet; we block writing input because it
    // hasn't read enough of it yet) — a real risk here, since the very point
    // of this function is to hand it a big `git log -p` stream. Write on a
    // separate thread so `wait_with_output` can drain stdout concurrently;
    // the thread exits (dropping `stdin`, closing the pipe so patch-id sees
    // EOF) whether or not the write fully succeeds.
    // trace:BUG-1288 | ai:claude
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let output = child.wait_with_output().ok()?;
    let _ = writer.join();
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    Some(
        stdout
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .map(str::to_string)
            .collect(),
    )
}

/// TASK-878: scan AIDA/Agent-tool managed worktrees and classify each under the
/// squash-aware safety model. Read-only — performs git ancestry/rev-list probes
/// and one forge merged-PR lookup per branch, never mutates anything. Returns a
/// `merged-agent-worktrees` finding for every agent worktree that is either
/// Removable (verified merged + clean + no unique commits; `safe_heal=false` so
/// removal stays gated behind --yes --force + the STORY-666 sign-off) or Keep
/// (flagged for the operator, never auto-removed). The project's own worktree
// and the `aida-store` worktree are skipped. trace:TASK-878
fn scan_merged_agent_worktrees(project_root: &std::path::Path) -> Vec<DoctorFinding> {
    use std::process::Command as PCmd;

    let git = |args: &[&str]| -> Option<u32> {
        PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<u32>()
                    .ok()
            })
    };

    // Without a resolvable default branch we cannot corroborate merges; stay
    // silent rather than risk flagging live worktrees.
    let Some(default_ref) = resolve_default_branch_ref(project_root) else {
        return Vec::new();
    };

    let project_canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());

    let mut findings = Vec::new();
    for wt in list_worktrees(project_root) {
        let wt_canon = wt.path.canonicalize().unwrap_or_else(|_| wt.path.clone());
        // Never touch the main worktree or the store worktree.
        if wt_canon == project_canon || wt.branch.as_deref() == Some("aida-store") {
            continue;
        }
        if !is_agent_managed_worktree(&wt.path, wt.branch.as_deref()) {
            continue;
        }
        let Some(branch) = wt.branch.as_deref() else {
            // A detached agent worktree carries no branch to verify against
            // origin/main — keep it; the operator can inspect by hand.
            findings.push(DoctorFinding {
                category: "merged-agent-worktrees".to_string(),
                id: wt.path.display().to_string(),
                summary: format!(
                    "agent worktree {} is detached (no branch) — flagged",
                    wt.path.display()
                ),
                action: "operator decision: inspect, then `git worktree remove` by hand"
                    .to_string(),
                safe_heal: false,
            });
            continue;
        };

        let dirty = !worktree_dirty_entries(&wt.path).is_empty();
        let ancestor_of_main = git(&["rev-list", "--count", &format!("{default_ref}..{branch}")])
            .map(|n| n == 0)
            .unwrap_or(false);
        let unique_unmerged_commits =
            git(&["rev-list", "--count", &format!("{default_ref}..{branch}")]).unwrap_or(0);
        // Only consult the forge when the cheap ancestry probe was inconclusive
        // (covers the squash-merge case) and the worktree is clean — a dirty
        // worktree is kept regardless, so skip the network call.
        let pr_merged = if !ancestor_of_main && !dirty {
            matches!(
                detect_merged_pr_for_branch_via_forge(project_root, branch),
                PrLookup::Found(_)
            )
        } else {
            false
        };

        // Only pay for the content probe when it could actually change the
        // verdict: a confirmed merged PR with a positive (ancestry-only)
        // commit count is exactly the case ancestry can never clear on its own.
        let content_fully_landed = if pr_merged && unique_unmerged_commits > 0 {
            branch_content_fully_landed(project_root, &default_ref, branch)
        } else {
            false
        };

        let facts = AgentWorktreeFacts {
            dirty,
            ancestor_of_main,
            pr_merged,
            unique_unmerged_commits,
            content_fully_landed,
        };

        match classify_agent_worktree(&facts) {
            AgentWorktreeVerdict::Removable(reason) => {
                findings.push(DoctorFinding {
                    category: "merged-agent-worktrees".to_string(),
                    id: wt.path.display().to_string(),
                    summary: format!(
                        "agent worktree {} on `{branch}` is mergeable-and-gone ({reason})",
                        wt.path.display()
                    ),
                    action: format!(
                        "remove worktree + delete branch `{branch}` (`aida doctor --heal \
                         --category merged-agent-worktrees --yes --force`)"
                    ),
                    // DESTRUCTIVE (worktree removal + branch deletion) → gated
                    // behind --yes --force AND the STORY-666 autonomous-context
                    // refusal in `heal_doctor_findings`.
                    safe_heal: false,
                });
            }
            AgentWorktreeVerdict::Keep(reason) => {
                findings.push(DoctorFinding {
                    category: "merged-agent-worktrees".to_string(),
                    id: wt.path.display().to_string(),
                    summary: format!(
                        "agent worktree {} on `{branch}` flagged: {reason}",
                        wt.path.display()
                    ),
                    action: "operator decision: review and keep, or remove by hand".to_string(),
                    // Never auto-removed — flag-only.
                    safe_heal: false,
                });
            }
        }
    }
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    findings
}

/// TASK-878: remove one merged agent worktree + delete its branch. Only
/// reachable when the finding surfaced it as Removable AND the caller passed
/// --yes --force under an interactive (non-autonomous) context — the STORY-666
/// destructive-heal gate in `heal_doctor_findings` fails closed otherwise. A
/// Keep-flagged worktree never routes here (its action string is operator-only).
/// Re-verifies the worktree is still clean right before removing (a salvage
/// patch is written if anything appeared since the scan) and only deletes the
// branch after the worktree is gone. trace:TASK-878 | ai:claude
fn heal_doctor_merged_agent_worktree(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    // Keep-flagged findings carry an operator-only action and must never be
    // auto-removed, even under --yes --force.
    if !finding.action.contains("--category merged-agent-worktrees") {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some("flagged for operator decision — not auto-removed".to_string()),
        });
    }
    let worktree = std::path::PathBuf::from(&finding.id);
    if !worktree.exists() {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some(format!("{} already gone", worktree.display())),
        });
    }

    // Resolve the worktree's branch so we can delete it after removal.
    let branch = list_worktrees(project_root)
        .into_iter()
        .find(|wt| {
            wt.path.canonicalize().unwrap_or_else(|_| wt.path.clone())
                == worktree.canonicalize().unwrap_or_else(|_| worktree.clone())
        })
        .and_then(|wt| wt.branch);

    // Re-check: never delete uncommitted work that appeared between scan and
    // heal. Salvage anything dirty, then refuse to remove. trace:TASK-878
    if !worktree_dirty_entries(&worktree).is_empty() {
        let salvage =
            salvage_worktree_patch(project_root, "merged-agent-worktree", None, &worktree)?;
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some(format!(
                "worktree became dirty since scan — NOT removed{}",
                salvage
                    .map(|p| format!(" (salvage patch: {})", p.display()))
                    .unwrap_or_default()
            )),
        });
    }

    // STORY-714: shared teardown — pre_destroy cargo-clean hook (TASK-0396)
    // fires before removal, and a pooled tree is deregistered from the registry.
    let removed = aida_core::worktree_pool_destroy::teardown_worktree_path(
        project_root,
        &worktree,
        &worktree_pool_global_hooks("pre_destroy"),
    );
    if removed.is_err() {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "failed".to_string(),
            detail: Some(format!("git worktree remove {} failed", worktree.display())),
        });
    }

    // Delete the now-unused local branch (the stale-branch source of the
    // `aida human` false-positives). `-D` because a squash-merged branch isn't
    // recognized as merged by `-d`, and the scan already verified it shipped.
    let mut detail = None;
    if let Some(branch) = branch {
        let deleted = std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["branch", "-D", &branch])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        detail = Some(if deleted {
            format!("removed worktree + deleted branch `{branch}`")
        } else {
            format!("removed worktree (branch `{branch}` delete failed or already gone)")
        });
    }

    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "removed merged agent worktree".to_string(),
        status: "healed".to_string(),
        detail,
    })
}

/// The heavy orientation diagnostics moved off bare `aida status`
/// and onto `aida doctor`. Prints the PR/CI status (a `gh` network call),
/// live-session/lease liveness + fleet roster, the Claude Code presence line,
/// the worktree pane, the open-PR roster, and cross-clone coordination claims —
/// the same section printers `aida status --full` uses. Constructs the cached
/// backend on demand (the doctor's own load goes through the legacy `Storage`
/// path) so `collect_user_context` has the cache-backed handle it needs. Each
/// section graceful-degrades on its own data; best-effort — a backend that can't
/// be opened simply prints nothing here rather than failing the doctor run.
// trace:STORY-707 | ai:claude
fn print_doctor_status_diagnostics(
    project_root: &std::path::Path,
    store: &aida_core::models::RequirementsStore,
) {
    let store_path = project_root.join(".aida-store");
    let Ok(dispenser) = load_dispenser(&store_path) else {
        return;
    };
    let Ok(inner) = aida_core::GitBackend::new(&store_path).map(|b| b.with_dispenser(dispenser))
    else {
        return;
    };
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let Ok(backend) = aida_core::CachedGitBackend::with_inner(inner, &cache_path) else {
        return;
    };

    println!();
    println!("{}", "─── Status diagnostics ───".bold());
    println!(
        "  {}",
        "(moved off `aida status` so the default snapshot stays instant — STORY-707)".dimmed()
    );
    println!();

    // The full user-context gather: PR/CI (gh), branch facts, queue snapshot,
    // and the live-session-probed agent roster. This is the ~16s the fast
    // `aida status` no longer pays — it lives here now.
    let user_ctx = collect_user_context(project_root, store, &backend, false);

    print_status_pr_section(&user_ctx, false);
    print_status_queue_section(&user_ctx, false);
    print_status_presence_line(project_root);
    print_status_agents_section(&user_ctx, true);
    print_status_claude_code_section(project_root);
    print_status_worktrees_section(project_root, true);
    print_status_open_prs_section(project_root, true);
    print_status_coordination_section(&store_path, chrono::Utc::now(), true);
}

fn render_doctor_report(report: &DoctorReport, healed: bool) -> Result<()> {
    println!("{}", "─── AIDA doctor ───".bold());
    if report.findings.is_empty() && report.hidden_completed_without_commit == 0 {
        println!(
            "  {} no multi-agent state drift detected",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    } else {
        let mut current = "";
        for finding in &report.findings {
            if current != finding.category {
                current = &finding.category;
                println!();
                println!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::Bullet).cyan(),
                    current.bold()
                );
            }
            let safety = if finding.safe_heal { "safe" } else { "manual" };
            println!("  - {} [{}]", finding.summary, safety.dimmed());
            println!("    → {}", finding.action.dimmed());
        }
        if report.hidden_completed_without_commit > 0 {
            if current != "completed-without-commit" {
                println!();
                println!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::Bullet).cyan(),
                    "completed-without-commit".bold()
                );
            }
            println!(
                "  ({} older completed-without-commit finding{} hidden — pass --all to list)",
                report.hidden_completed_without_commit,
                if report.hidden_completed_without_commit == 1 {
                    ""
                } else {
                    "s"
                }
            );
        }
        println!();
        println!(
            "  Total: {} visible finding{}",
            report.findings.len(),
            if report.findings.len() == 1 { "" } else { "s" }
        );
    }

    if healed {
        println!();
        println!("{}", "─── Heal results ───".bold());
        if report.healed.is_empty() {
            println!("  (no heal actions applied)");
        }
        for result in &report.healed {
            println!(
                "  - {} {}: {}",
                result.status.as_str().bold(),
                result.id,
                result.action
            );
            if let Some(detail) = &result.detail {
                println!("    {}", detail.dimmed());
            }
        }
    } else if !report.findings.is_empty() {
        println!("  Re-run with {} to apply safe fixes.", "--heal".cyan());
    }

    // TASK-865: report read-only environment facts the operator wants when
    // triaging launch confinement — currently the bubblewrap OS-sandbox status.
    // Availability only; this never enables `os_wrap`. trace:TASK-865 | ai:claude
    println!();
    println!("{}", "─── Environment ───".bold());
    render_doctor_bwrap_row();
    render_doctor_forge_row();
    Ok(())
}

/// Render the forge-CLI availability row in `aida doctor`'s environment section.
/// `aida pr auto-queue-review` (and the rest of the PR/CI lifecycle) fails hard
/// when the project's forge CLI (`gh` for GitHub, `glab` for GitLab) is not on
/// PATH, so surface it here — OK / missing-with-install-hint / none-needed
/// (pure-git) — rather than letting it stay silent until the first `pr` command.
// Colourised to match the doctor-check output style. trace:TASK-860 | ai:claude
fn render_doctor_forge_row() {
    let project_root = match find_project_root() {
        Ok(root) => main_worktree_root_from(&root),
        // No project context (e.g. run outside a repo) — fall back to CWD so the
        // row still reports something useful rather than panicking.
        Err(_) => std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    };
    let (kind, msg) = crate::forge::forge_cli_status(&project_root);
    let glyph = if kind == crate::forge::ForgeKind::None {
        // Pure-git needs no forge CLI — informational, not a warning.
        crate::glyph(crate::glyphs::Glyph::Bullet).dimmed()
    } else if kind.cli_on_path() {
        crate::glyph(crate::glyphs::Glyph::Check).green()
    } else {
        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
    };
    println!("  {} {}", glyph, msg);
}

/// Render the bwrap availability row in `aida doctor`'s environment section,
/// colourised to match the doctor-check output style. When confinement is
/// blocked or bwrap is missing, print the EXACT copy-pasteable remediation
/// (not just a one-line prose hint) plus a pointer at the guided setup command;
// when it's ready, confirm how to opt in. trace:TASK-865 | ai:claude
// trace:STORY-665 | ai:claude
fn render_doctor_bwrap_row() {
    let avail = crate::session::bwrap_availability();
    let glyph = match avail {
        crate::session::BwrapAvailability::Ok => crate::glyph(crate::glyphs::Glyph::Check).green(),
        crate::session::BwrapAvailability::NotInstalled => {
            crate::glyph(crate::glyphs::Glyph::Bullet).dimmed()
        }
        crate::session::BwrapAvailability::UsernsBlocked { .. } => {
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        }
    };
    println!("  {} {}", glyph, bwrap_status_line());
    match avail {
        crate::session::BwrapAvailability::Ok => {
            // Ready — confirm + point at how to opt in (it's off by default).
            println!(
                "    {}",
                "OS sandbox is ready. Opt in per-host with `export AIDA_OS_WRAP=1` (recommended — \
                 no shared-config change), or repo-wide with [contained] os_wrap = true in .aida/config.toml."
                    .dimmed()
            );
        }
        crate::session::BwrapAvailability::NotInstalled => {
            // Exact install command, then the guided-setup pointer.
            println!("    {}", "Install bubblewrap, then enable:".dimmed());
            println!("      {}", crate::session::BWRAP_INSTALL_DEBIAN.cyan());
            println!(
                "    {}",
                "Then run `aida doctor --fix-sandbox` for the full setup steps.".dimmed()
            );
        }
        crate::session::BwrapAvailability::UsernsBlocked { .. } => {
            // The kernel blocks unprivileged userns — print the EXACT runtime +
            // persist sysctl commands, clearly marked as sudo. trace:STORY-665
            println!(
                "    {}",
                "Kernel blocks unprivileged user namespaces. Fix (run these yourself):".dimmed()
            );
            println!(
                "      {}  {}",
                crate::session::BWRAP_USERNS_SYSCTL_RUNTIME.cyan(),
                "# this boot".dimmed()
            );
            println!(
                "      {}  {}",
                crate::session::BWRAP_USERNS_SYSCTL_PERSIST.cyan(),
                "# persist".dimmed()
            );
            println!(
                "    {}",
                "Then run `aida doctor --fix-sandbox` for the full setup + verify steps.".dimmed()
            );
        }
    }
}

/// `aida doctor --fix-sandbox` — guided, copy-pasteable bring-up of the OS
/// sandbox (bubblewrap write-confinement) on the current host. A PRINTER, not a
/// silent sudo-runner: it detects the current state, prints the exact ordered
/// sequence (install / persist-sysctl / opt-in / verify) with sudo steps marked
/// "run this yourself", and runs the NON-sudo availability re-probe as a smoke
/// check. Honest + safe — it never escalates privileges on the user's behalf.
// trace:STORY-665 | ai:claude
pub(crate) fn doctor_fix_sandbox() -> Result<()> {
    use crate::session::{
        bwrap_availability, BwrapAvailability, BWRAP_INSTALL_DEBIAN, BWRAP_USERNS_SYSCTL_PERSIST,
        BWRAP_USERNS_SYSCTL_RUNTIME,
    };

    println!(
        "{}",
        "Guided OS-sandbox setup (bubblewrap write-confinement)"
            .bold()
            .cyan()
    );
    println!(
        "{}",
        "AIDA can run the agent it launches under an OS-level write-confinement \n\
         sandbox (bwrap). It is opt-in and off by default. This walks you through \n\
         bringing it up on THIS host. sudo steps are yours to run."
            .dimmed()
    );
    println!();

    let avail = bwrap_availability();

    // Step 1 — detected state.
    println!("{}", "1. Detected state".bold());
    match &avail {
        BwrapAvailability::Ok => println!(
            "   {} bwrap is installed and the unprivileged-userns self-test passes.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        ),
        BwrapAvailability::NotInstalled => println!(
            "   {} bwrap is not installed on this host.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        ),
        BwrapAvailability::UsernsBlocked { .. } => println!(
            "   {} bwrap is installed, but the kernel is blocking the unprivileged \n      user namespace it needs (Ubuntu 23.10+/24.04 AppArmor default).",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        ),
    }
    println!();

    let mut step = 2;

    // Step 2 — install (only if missing).
    if avail == BwrapAvailability::NotInstalled {
        println!("{}", format!("{step}. Install bubblewrap").bold());
        println!("   {}", "Run this yourself (sudo):".dimmed());
        println!("      {}", BWRAP_INSTALL_DEBIAN.cyan());
        println!(
            "   {}",
            "(non-Debian: use your package manager's `bubblewrap` package)".dimmed()
        );
        println!();
        step += 1;
    }

    // Step (conditional) — lift + persist the userns sysctl.
    if matches!(avail, BwrapAvailability::UsernsBlocked { .. })
        || avail == BwrapAvailability::NotInstalled
    {
        println!(
            "{}",
            format!("{step}. Permit unprivileged user namespaces").bold()
        );
        println!(
            "   {}",
            "Run these yourself (sudo). First applies it now, second persists across reboots:"
                .dimmed()
        );
        println!(
            "      {}  {}",
            BWRAP_USERNS_SYSCTL_RUNTIME.cyan(),
            "# this boot".dimmed()
        );
        println!(
            "      {}  {}",
            BWRAP_USERNS_SYSCTL_PERSIST.cyan(),
            "# persist".dimmed()
        );
        println!();
        step += 1;
    }

    // Step — opt in (the knob is off by default regardless of host state).
    println!("{}", format!("{step}. Enable the sandbox (opt-in)").bold());
    println!(
        "   {}",
        "Set the knob in .aida/config.toml (off by default):".dimmed()
    );
    println!("      {}", "[contained]".cyan());
    println!("      {}", "os_wrap = true".cyan());
    println!(
        "   {}",
        "Recommended: enable per-host with `export AIDA_OS_WRAP=1` (no shared-config change). The config knob above enables it repo-wide.".dimmed()
    );
    println!();
    step += 1;

    // Step — verify.
    println!("{}", format!("{step}. Verify").bold());
    println!(
        "   {}",
        "Re-run this command (or `aida doctor`) — step 1 should report a passing self-test:"
            .dimmed()
    );
    println!("      {}", "aida doctor --fix-sandbox".cyan());
    println!(
        "   {}",
        "Once enabled, `aida config show` renders the resolved [contained] posture.".dimmed()
    );
    println!();

    // Non-sudo smoke: re-probe availability and report the live verdict so the
    // operator sees whether the host is already there without touching sudo.
    println!("{}", "Live smoke check (non-sudo re-probe)".bold());
    match bwrap_availability() {
        BwrapAvailability::Ok => println!(
            "   {} Confinement self-test PASSES — this host is ready; just set os_wrap = true.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        ),
        BwrapAvailability::NotInstalled => println!(
            "   {} bwrap still not on PATH — run the install step above.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        ),
        BwrapAvailability::UsernsBlocked { .. } => println!(
            "   {} Self-test still failing — run the sudo sysctl step above, then re-check.",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        ),
    }
    println!();
    println!(
        "   {}",
        "Full reference: docs/agents/claude-bubblewrap-sandbox.md".dimmed()
    );

    Ok(())
}

/// STORY-666: is THIS process an autonomous / unattended context for the
/// purposes of the destructive-heal gate? Pure so it is unit-testable without
/// touching the real terminal or environment — `doctor_running_autonomously`
/// (below) feeds it the live signals.
///
/// **Autonomous = no interactive TTY OR a corroborated live orchestrator run.**
///
/// Deliberately NOT keyed on `--yes` alone: a human at a keyboard typing
/// `aida doctor --heal --force --yes` IS the explicit sign-off, and that
/// interactive path must stay exactly as it is today (STORY-666 req #3). The
/// `--yes` flag only ever *reaches* a destructive heal together with `--force`,
/// and at a TTY that combination is a deliberate human decision; what we must
/// fail-closed against is the genuinely unattended case — piped/CI stdin (no
// TTY) or an `--auto-complete` drain (orchestrator token live). trace:STORY-666
fn doctor_context_is_autonomous(stdin_is_tty: bool, orchestrated: bool) -> bool {
    !stdin_is_tty || orchestrated
}

/// STORY-666: live-signal wrapper for [`doctor_context_is_autonomous`] — reads
/// the real TTY state + the corroborated orchestrator verdict for `project_root`.
// trace:STORY-666 | ai:claude
fn doctor_running_autonomously(project_root: &std::path::Path) -> bool {
    let orchestrated = orchestrator::detect(project_root).is_orchestrated();
    doctor_context_is_autonomous(std::io::stdin().is_terminal(), orchestrated)
}

/// STORY-666: the heal disposition for one finding-category, decided by its
/// safe/destructive classification, the requested flags, and whether we are in
/// an autonomous context. Pure → unit-testable. The single place the
// fail-closed invariant lives. trace:STORY-666 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HealDisposition {
    /// Apply the category's heals (safe always; destructive only with explicit
    /// interactive sign-off).
    Proceed,
    /// A destructive category was requested via `--force --yes` but we are in a
    /// non-interactive/autonomous context — refuse and report (fail-closed).
    GateAutonomous,
    /// A destructive category WITHOUT the `--force --yes` opt-in — the
    /// pre-existing "needs a manual decision" skip (unchanged).
    SkipNeedsForce,
}

fn doctor_heal_disposition(
    safe: bool,
    force: bool,
    yes: bool,
    autonomous: bool,
) -> HealDisposition {
    if safe {
        // Safe, reversible heals proceed in every context — the safe
        // classification IS the bouncer. Never over-gate routine fixes.
        return HealDisposition::Proceed;
    }
    // Destructive from here down.
    if !(force && yes) {
        return HealDisposition::SkipNeedsForce;
    }
    if autonomous {
        // Fail-closed: a destructive heal must not execute silently with no one
        // to make the check-before-delete judgment.
        return HealDisposition::GateAutonomous;
    }
    // Destructive + --force --yes + interactive TTY = explicit human sign-off.
    HealDisposition::Proceed
}

fn heal_doctor_findings(
    project_root: &std::path::Path,
    findings: &[DoctorFinding],
    opts: &DoctorRunOptions,
) -> Result<Vec<DoctorHealResult>> {
    // STORY-666: detect the autonomous/unattended context ONCE for the whole
    // run — the destructive-heal gate keys off it. trace:STORY-666 | ai:claude
    let autonomous = doctor_running_autonomously(project_root);
    let mut out = Vec::new();
    let mut by_category: std::collections::BTreeMap<String, Vec<&DoctorFinding>> =
        std::collections::BTreeMap::new();
    for finding in findings {
        by_category
            .entry(finding.category.clone())
            .or_default()
            .push(finding);
    }
    let mut categories: Vec<_> = by_category.into_iter().collect();
    categories.sort_by_key(|(category, _)| doctor_heal_category_order(category));
    for (category, items) in categories {
        let safe = items.iter().all(|item| item.safe_heal);
        match doctor_heal_disposition(safe, opts.force, opts.yes, autonomous) {
            HealDisposition::Proceed => {}
            HealDisposition::SkipNeedsForce => {
                out.push(DoctorHealResult {
                    category: category.clone(),
                    id: category.clone(),
                    action: "skipped category requiring manual decision".to_string(),
                    status: "skipped".to_string(),
                    detail: Some(
                        "pass --yes --force only when you want destructive branch cleanup".into(),
                    ),
                });
                continue;
            }
            HealDisposition::GateAutonomous => {
                // STORY-666: fail-closed. A destructive heal was requested
                // (--force --yes) but we are unattended (no TTY / live drain).
                // Refuse it, name exactly what was skipped, and print the precise
                // interactive command to run it under human sign-off. The
                // `skipped` status keeps the audit trail legible and the heal
                // never executes. trace:STORY-666 | ai:claude
                out.push(DoctorHealResult {
                    category: category.clone(),
                    id: category.clone(),
                    action: format!(
                        "gated — destructive heal of {} finding(s) withheld (unattended context)",
                        items.len()
                    ),
                    status: "skipped".to_string(),
                    detail: Some(format!(
                        "destructive fixes require sign-off and were NOT applied in this \
                         unattended context. Re-run it at an interactive terminal: \
                         `aida doctor --heal --force --yes --category {category}`"
                    )),
                });
                continue;
            }
        }
        if !opts.yes && !confirm_doctor_category(&category, items.len())? {
            out.push(DoctorHealResult {
                category: category.clone(),
                id: category.clone(),
                action: "operator declined".to_string(),
                status: "skipped".to_string(),
                detail: None,
            });
            continue;
        }
        for finding in items {
            // BUG-471: a single finding's heal failure must not abort the whole
            // run (the resilient-drain discipline). Record it as a `failed`
            // result and continue; the caller surfaces failures + exits non-zero.
            // trace:BUG-471 | ai:claude
            match heal_doctor_finding(project_root, finding, opts) {
                Ok(result) => out.push(result),
                Err(e) => out.push(DoctorHealResult {
                    category: category.clone(),
                    id: finding.id.clone(),
                    action: finding.action.clone(),
                    status: "failed".to_string(),
                    detail: Some(e.to_string()),
                }),
            }
        }
    }
    Ok(out)
}

fn doctor_heal_category_order(category: &str) -> u8 {
    match category {
        // trace:BUG-1136 | ai:codex
        // Lease reaping has to precede status derivation in an all-category
        // heal. Otherwise an Approved spec with only a stale lease can be
        // bumped to In Progress, then lose the lease later in the same pass.
        "stale-leases" | "abandoned-leases" | "stale-reviewer-leases" => 0,
        "spec-status-drift" => 1,
        _ => 2,
    }
}

fn confirm_doctor_category(category: &str, count: usize) -> Result<bool> {
    // BUG-407: never block on a prompt nobody can answer. In a non-interactive
    // shell (no TTY — a background task, CI, or piped stdin) `stdin.read_line`
    // blocks forever on an open-but-empty socket (the observed `aida doctor
    // --heal` hang: 7.5min, 0 progress, WCHAN unix_stream_read_generic).
    // Decline fast with guidance instead; `--heal --yes` skips this prompt
    // entirely (the caller only calls us when !opts.yes). trace:BUG-407
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "  {} non-interactive shell — skipping '{}' ({} finding(s)). Re-run \
             `aida doctor --heal --yes` (add --force for destructive categories) \
             to apply without prompting.",
            "Note:".yellow().bold(),
            category,
            count
        );
        return Ok(false);
    }
    // trace:STORY-809 | ai:claude
    let card = crate::context_prompt::ContextCard {
        decision: format!("whether doctor may heal the '{category}' findings"),
        provenance: vec![format!(
            "`aida doctor` detected {count} finding(s) in category '{category}'"
        )],
        answers: vec![
            format!("y: apply the '{category}' heal to all {count} finding(s) now"),
            "n: skip this category (findings remain; re-run doctor any time)".to_string(),
        ],
        recommended_default:
            "y for hygiene categories (stale leases, dead registrations); read the findings first for destructive ones"
                .to_string(),
    };
    crate::context_prompt::confirm_with_context(
        &format!("Heal {count} finding(s) in {category}?"),
        false,
        &card,
    )
}

fn heal_doctor_finding(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
    opts: &DoctorRunOptions,
) -> Result<DoctorHealResult> {
    match finding.category.as_str() {
        "stale-leases" | "abandoned-leases" | "stale-reviewer-leases" => {
            heal_doctor_lease(project_root, finding)
        }
        "brief-spec-drift" | "OBE-briefs" => heal_doctor_brief(finding),
        "spec-status-drift" if finding.action.starts_with("confirm and end lease") => {
            heal_doctor_lease(project_root, finding)
        }
        "spec-status-drift" => heal_doctor_spec_status(project_root, finding),
        "orphan-worktrees" => heal_doctor_orphan_worktree(project_root, finding),
        // TASK-752: git-rm the tracked legacy-store artifact + gitignore it —
        // exactly PR-651's resolution. safe_heal (the detector self-gates on
        // git-canonical mode, so this never touches an active centralized
        // backend). trace:TASK-752 | ai:claude
        "legacy-store-cruft" => heal_doctor_legacy_store_cruft(project_root, finding),
        // BUG-563: git-rm --cached the per-clone runtime file in the STORE
        // worktree + gitignore it + commit on the store worktree. safe_heal (the
        // detector self-gates on git-canonical mode + an attached store
        // worktree). trace:BUG-563 | ai:claude
        "store-tracked-runtime" => heal_doctor_store_tracked_runtime(project_root, finding),
        "orphan-queue-entries" => heal_doctor_orphan_queue_entry(project_root, finding),
        "stale-locks" => heal_doctor_stale_lock(finding),
        "dead-agents" => heal_doctor_dead_agent(project_root, finding),
        // STORY-1128: permission-posture heal applies the contained profile
        // through the same merge-preserving writer as `config permissions set`.
        // trace:STORY-1128 | ai:codex
        "permission-posture" => heal_doctor_permission_posture(project_root, finding),
        "parent-tag-drift" => heal_doctor_parent_tag_drift(project_root, finding),
        "orphan-branches" if opts.force && opts.yes => {
            heal_doctor_orphan_branch(project_root, finding)
        }
        // TASK-717: prune a verified-stale REMOTE branch. DESTRUCTIVE (deletes
        // origin/*) so force+yes gated like local orphan-branch deletion.
        // trace:TASK-717
        "stale-remote-branches" if opts.force && opts.yes => {
            heal_doctor_stale_remote_branch(project_root, finding)
        }
        // TASK-878: GC a merged Agent-tool worktree + its branch. DESTRUCTIVE
        // (worktree removal + branch deletion) so force+yes gated like the
        // stale-remote-branch deletion above, AND routed through the STORY-666
        // autonomous-context refusal in heal_doctor_findings (safe_heal=false).
        // trace:TASK-878
        "merged-agent-worktrees" if opts.force && opts.yes => {
            heal_doctor_merged_agent_worktree(project_root, finding)
        }
        // TASK-673 / BUG-1637: the heal reports and refuses; it never reopens
        // the Completed spec (see heal_doctor_completed_without_commit).
        // Still force-gated (see heal_doctor_findings).
        "completed-without-commit" if opts.force && opts.yes => {
            heal_doctor_completed_without_commit(project_root, finding)
        }
        // TASK-699: remove a stray ancestor instruction file whose @-imports
        // escape the project. DESTRUCTIVE — it deletes a file OUTSIDE the repo —
        // so it's force+yes gated (see heal_doctor_findings's safe-heal gate).
        // trace:TASK-699
        "external-import-bleed" if opts.force && opts.yes => {
            heal_doctor_external_import_bleed(project_root, finding)
        }
        _ => Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some("diagnostic-only category; follow the printed action".to_string()),
        }),
    }
}

// Relationship edges are the source of truth; rebuild only the denormalized
// parent:* tags for this requirement. trace:BUG-1252 | ai:codex
pub(crate) fn heal_doctor_parent_tag_drift(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let storage = Storage::new(project_root.join(".aida-store"));
    let mut store = storage.load()?;
    let req_id = store
        .get_requirement_by_spec_id(&finding.id)
        .context("parent-tag-drift requirement disappeared")?
        .id;
    let expected: Vec<String> = store
        .get_requirement_by_id(&req_id)
        .into_iter()
        .flat_map(|req| req.relationships.iter())
        .filter(|rel| rel.rel_type == aida_core::models::RelationshipType::Child)
        .filter_map(|rel| store.requirements.iter().find(|r| r.id == rel.target_id))
        .filter_map(|parent| parent.spec_id.as_deref())
        .map(|id| format!("parent:{id}"))
        .collect();
    let req = store
        .get_requirement_by_id_mut(&req_id)
        .context("requirement disappeared")?;
    req.tags.retain(|tag| !tag.starts_with("parent:"));
    req.tags.extend(expected);
    storage.save(&store)?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: finding.action.clone(),
        status: "healed".to_string(),
        detail: None,
    })
}

// trace:STORY-1128 | ai:codex
fn heal_doctor_permission_posture(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let result = crate::config_cmd::apply_permission_posture(
        project_root,
        cli::ConfigPermissionTier::Contained,
        crate::config_cmd::PermissionPostureScope::Local,
    )?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "applied contained permission posture".to_string(),
        status: "healed".to_string(),
        detail: Some(format!(
            "wrote {}; backups {}",
            result
                .edited_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            result
                .backup_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    })
}

/// STORY-496: reap a stale agent-registry entry. The finding id is
/// `{agent_type}#{pid}`. Re-checks registry liveness before removing — a pid can
/// be reused, and role-enter shell seats can go stale while the shell pid lives,
/// so heal must share the status predicate.
// trace:STORY-496 | ai:claude
// trace:BUG-1156 | ai:codex
fn heal_doctor_dead_agent(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let (agent_type, pid_str) = finding.id.split_once('#').ok_or_else(|| {
        anyhow::anyhow!(
            "malformed dead-agent id `{}` (expected type#pid)",
            finding.id
        )
    })?;
    let pid: u32 = pid_str
        .parse()
        .with_context(|| format!("bad pid in dead-agent id `{}`", finding.id))?;
    if agent_registry::agent_is_live(project_root, agent_type, pid) {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some(format!(
                "agent {agent_type}#{pid} is now live — not reaping"
            )),
        });
    }
    let removed = agent_registry::remove_agent(project_root, agent_type, pid)?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: finding.action.clone(),
        status: if removed { "healed" } else { "skipped" }.to_string(),
        detail: if removed {
            None
        } else {
            Some("registry entry already gone".to_string())
        },
    })
}

/// TASK-699: opt-in heal for the `external-import-bleed` category — remove a
/// stray ancestor CLAUDE.md / CLAUDE.local.md / AGENTS.md whose @-imports escape
/// the project (the classic accidental-`aida init`-in-a-parent-of-projects
/// scaffold). DESTRUCTIVE: it deletes a file OUTSIDE the repo, so the caller
/// gates it behind --heal --force --yes. Guardrails honored here:
///   - removes ONLY the stray instruction file (the finding id), never the
///     ancestor's docs/ or anything else;
///   - re-reads the file and re-verifies at least one @-import STILL escapes the
///     project right before deleting (mirrors heal_doctor_dead_agent's re-check
///     so a file edited/fixed between scan and heal is left alone);
///   - prints exactly what will be deleted before removing it.
// trace:TASK-699 | ai:claude
fn heal_doctor_external_import_bleed(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let file = std::path::PathBuf::from(&finding.id);
    // Re-check: the file may have been removed or edited since the scan.
    let Ok(content) = std::fs::read_to_string(&file) else {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some(format!("{} already gone", file.display())),
        });
    };
    let file_dir = file.parent().unwrap_or_else(|| std::path::Path::new("/"));
    if !external_import_bleed::file_still_escapes(file_dir, &content, project_root) {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some(format!(
                "{} no longer has @-imports escaping the project — not removing",
                file.display()
            )),
        });
    }
    // Print EXACTLY what will be deleted before acting (guardrail #4).
    println!(
        "  {} removing stray ancestor instruction file: {}",
        "→".red().bold(),
        file.display()
    );
    std::fs::remove_file(&file).with_context(|| format!("failed to remove {}", file.display()))?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: format!("removed stray ancestor instruction file {}", file.display()),
        status: "healed".to_string(),
        detail: None,
    })
}

fn heal_doctor_lease(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    // BUG-471: a lease already ended (e.g. removed earlier in this same heal
    // run, or by a concurrent session) is a no-op, not an error — mirror the
    // dead-agent "already gone" handling so heal stays idempotent.
    // trace:BUG-471 | ai:claude
    let Some(lease) = list_leases(project_root)
        .into_iter()
        .find(|lease| lease.id == finding.id)
    else {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some("lease already ended".to_string()),
        });
    };
    let salvage = salvage_worktree_patch(
        project_root,
        &lease.scope,
        lease.role.as_deref(),
        &lease.worktree_path,
    )?;
    let removed = force_cleanup_lease(project_root, &lease);
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "ended lease with salvage-first cleanup".to_string(),
        status: if removed { "healed" } else { "partial" }.to_string(),
        detail: salvage.map(|p| format!("salvage patch: {}", p.display())),
    })
}

fn heal_doctor_brief(finding: &DoctorFinding) -> Result<DoctorHealResult> {
    let path = std::path::Path::new(&finding.id);
    if !path.exists() {
        let acked = path.with_file_name(format!(
            "{}.acked",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
        ));
        if acked.exists() {
            return Ok(DoctorHealResult {
                category: finding.category.clone(),
                id: finding.id.clone(),
                action: "brief already acked".to_string(),
                status: "skipped".to_string(),
                detail: Some(acked.display().to_string()),
            });
        }
    }
    ack_agent_brief(path)?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "acked obsolete brief".to_string(),
        status: "healed".to_string(),
        detail: None,
    })
}

fn heal_doctor_spec_status(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let storage = Storage::new(project_root.join(".aida-store"));
    let mut store = storage.load()?;
    let current_scope_lease = list_leases(project_root)
        .iter()
        .any(|lease| lease.scope.eq_ignore_ascii_case(&finding.id));
    let mut action = None;
    for req in &mut store.requirements {
        if req.spec_id.as_deref() != Some(finding.id.as_str()) {
            continue;
        }
        if matches!(req.status, RequirementStatus::Approved)
            && finding.action.starts_with("bump spec to In Progress")
        {
            // trace:BUG-1136 | ai:codex
            // Re-read leases at heal time. The status finding was collected
            // before earlier heal categories may have reaped stale leases, so
            // the original action can be obsolete within this same command.
            if !current_scope_lease {
                continue;
            }
            // BUG-1637: an automated repair, recorded under the doctor's
            // author so the BUG-1625 merge guard can see it.
            // trace:BUG-1637 | ai:claude
            aida_core::conflict::set_status_recorded(
                req,
                RequirementStatus::InProgress,
                aida_core::conflict::DOCTOR_AUTHOR,
            );
            req.modified_at = chrono::Utc::now();
            action = Some("bumped Approved spec to In Progress".to_string());
        } else if matches!(req.status, RequirementStatus::InProgress)
            && finding.action.contains("no active lease")
        {
            if current_scope_lease {
                continue;
            }
            // trace:BUG-1637 | ai:claude
            aida_core::conflict::set_status_recorded(
                req,
                RequirementStatus::Approved,
                aida_core::conflict::DOCTOR_AUTHOR,
            );
            req.modified_at = chrono::Utc::now();
            action = Some(format!(
                "reverted {} from In Progress to Approved (no active lease found)",
                finding.id
            ));
        }
    }
    if action.is_some() {
        storage.save(&store)?;
    }
    let changed = action.is_some();
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: action.unwrap_or_else(|| "no automatic status change applied".to_string()),
        status: if changed { "healed" } else { "skipped" }.to_string(),
        detail: None,
    })
}

/// TASK-673 / BUG-1637: the completed-without-commit "heal" REFUSES to move
/// the spec and reports instead.
///
/// It used to re-open the Completed spec to Done. That is an automated write
/// leaving a terminal status on the strength of a heuristic: "git cannot find a
/// corroborating commit" is also what a stale or shallow clone, a squash merge
/// that dropped the trailer, or work landed in another repository look like.
/// Exactly that stale-clone shape is the BUG-1625 incident, and the merge guard
/// exists to stop an automated source from regressing a terminal status. So
/// the doctor never reopens a terminal status. A person who has checked the
/// spec reopens it with `aida edit <ID> --status done --force`, which records
/// the change under their own name.
// trace:TASK-673 trace:BUG-1637 | ai:claude
fn heal_doctor_completed_without_commit(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let storage = Storage::new(project_root.join(".aida-store"));
    let store = storage.load()?;
    let still_completed = store.requirements.iter().any(|req| {
        (req.spec_id.as_deref() == Some(finding.id.as_str())
            || req.agreed_id.as_deref() == Some(finding.id.as_str()))
            && matches!(req.status, RequirementStatus::Completed)
    });
    let (action, detail) = if still_completed {
        (
            format!(
                "left {} Completed: doctor never reopens a terminal status",
                finding.id
            ),
            Some(format!(
                "git found no commit for {id}, but a stale clone or a squash merge looks the \
                 same. Check it; if the work really did not land, reopen it yourself with \
                 `aida edit {id} --status done --force`.",
                id = finding.id
            )),
        )
    } else {
        (
            "spec is no longer Completed — nothing to report".to_string(),
            None,
        )
    };
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action,
        status: "skipped".to_string(),
        detail,
    })
}

fn heal_doctor_orphan_worktree(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let worktree = std::path::PathBuf::from(&finding.id);
    let salvage = salvage_worktree_patch(project_root, "orphan-worktree", None, &worktree)?;
    // STORY-714: route through the shared teardown so the pre_destroy
    // cargo-clean hook fires (TASK-0396) and a pooled tree is deregistered.
    let healed = aida_core::worktree_pool_destroy::teardown_worktree_path(
        project_root,
        &worktree,
        &worktree_pool_global_hooks("pre_destroy"),
    )
    .is_ok();
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "removed orphan worktree".to_string(),
        status: if healed { "healed" } else { "failed" }.to_string(),
        detail: salvage.map(|p| format!("salvage patch: {}", p.display())),
    })
}

/// TASK-752: heal a tracked legacy-store-cruft finding — exactly PR-651's
/// resolution: `git rm` the file from the tree, then append the gitignore block
/// (`requirements*.yaml`, `scaffold-report.html`) so the artifacts can't return.
/// Idempotent: a path already untracked (e.g. a prior heal removed it) reports
// `skipped`; the gitignore block is appended only once. trace:TASK-752 | ai:claude
fn heal_doctor_legacy_store_cruft(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let path = &finding.id;

    // Re-confirm the file is still tracked before removing — between scan and
    // heal another heal (or a manual `git rm`) may have already dropped it.
    let still_tracked = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["ls-files", "--error-unmatch", path])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !still_tracked {
        // Still make sure the gitignore guard is in place (idempotent).
        let gi = ensure_legacy_store_cruft_gitignore(project_root)?;
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: "already untracked".to_string(),
            status: "skipped".to_string(),
            detail: gi.then(|| "appended gitignore guard".to_string()),
        });
    }

    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rm", "--quiet", "--"])
        .arg(path)
        .status()
        .with_context(|| format!("git rm {path}"))?;
    if !status.success() {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "failed".to_string(),
            detail: Some(format!("git rm {path} failed")),
        });
    }

    let appended = ensure_legacy_store_cruft_gitignore(project_root)?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "git rm + gitignore (live store is the orphan aida-store branch)".to_string(),
        status: "healed".to_string(),
        detail: appended.then(|| "appended gitignore guard".to_string()),
    })
}

/// TASK-752: append PR-651's exact gitignore guard so the swept legacy-store
/// artifacts can't return. Idempotent — no-op if the patterns are already
// present. Returns whether it wrote anything. trace:TASK-752 | ai:claude
fn ensure_legacy_store_cruft_gitignore(project_root: &std::path::Path) -> Result<bool> {
    use std::io::Write;
    let gitignore_path = project_root.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
    // Already guarded? `requirements*.yaml` is the load-bearing pattern.
    if existing.lines().any(|l| l.trim() == "requirements*.yaml") {
        return Ok(false);
    }
    let block = "\n# Legacy pre-git-canonical store snapshots (live store is the orphan aida-store branch)\n\
         requirements*.yaml\n\
         scaffold-report.html\n";
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore_path)
        .with_context(|| format!("opening {}", gitignore_path.display()))?;
    f.write_all(block.as_bytes())
        .with_context(|| format!("appending to {}", gitignore_path.display()))?;
    Ok(true)
}

/// BUG-563: heal a per-clone runtime file wrongly tracked on the orphan
/// `aida-store` branch — `git rm --cached` it IN THE STORE WORKTREE (untrack but
/// keep the working copy, since the live clone still needs its own node.toml /
/// dispenser.toml / cache), ensure the store-worktree gitignore guards the
/// per-clone runtime set, then COMMIT the untrack on the store worktree so the
/// orphan branch stops carrying it. Idempotent: a path already untracked (e.g. a
/// prior heal removed it) reports `skipped`; the gitignore block is appended only
// once; a no-op commit is skipped. trace:BUG-563 | ai:claude
fn heal_doctor_store_tracked_runtime(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let store_worktree = project_root.join(".aida-store");
    let path = &finding.id;

    // Re-confirm the file is still tracked on the orphan branch before removing —
    // between scan and heal another heal (or a manual `git rm`) may have already
    // dropped it.
    let still_tracked = std::process::Command::new("git")
        .arg("-C")
        .arg(&store_worktree)
        .args(["ls-files", "--error-unmatch", path])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !still_tracked {
        // Still make sure the gitignore guard is in place (idempotent).
        let gi = ensure_store_tracked_runtime_gitignore(&store_worktree)?;
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: "already untracked".to_string(),
            status: "skipped".to_string(),
            detail: gi.then(|| "appended gitignore guard".to_string()),
        });
    }

    // Untrack but KEEP the working copy — this clone still needs its own
    // per-clone runtime file; only the orphan branch must stop carrying it.
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(&store_worktree)
        .args(["rm", "--cached", "--quiet", "--"])
        .arg(path)
        .status()
        .with_context(|| format!("git rm --cached {path} in store worktree"))?;
    if !status.success() {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "failed".to_string(),
            detail: Some(format!("git rm --cached {path} failed")),
        });
    }

    let appended = ensure_store_tracked_runtime_gitignore(&store_worktree)?;

    // Stage the gitignore (if we touched it) and commit the untrack on the
    // store worktree so the orphan branch stops carrying the per-clone file.
    if appended {
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(&store_worktree)
            .args(["add", ".gitignore"])
            .status();
    }
    let commit = std::process::Command::new("git")
        .arg("-C")
        .arg(&store_worktree)
        .args([
            "commit",
            "--quiet",
            "-m",
            "chore(store): stop tracking per-clone runtime file (BUG-563)",
        ])
        .status()
        .with_context(|| "committing untrack on store worktree".to_string())?;
    if !commit.success() {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "failed".to_string(),
            detail: Some("commit on store worktree failed".to_string()),
        });
    }

    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "git rm --cached + gitignore + commit on store worktree (per-clone runtime stays untracked)"
            .to_string(),
        status: "healed".to_string(),
        detail: appended.then(|| "appended gitignore guard".to_string()),
    })
}

/// BUG-563: append the per-clone-runtime gitignore guard to the STORE worktree's
/// `.gitignore` so the untracked files can't return on the orphan branch.
/// Idempotent — no-op if the load-bearing patterns are already present. Returns
// whether it wrote anything. trace:BUG-563 | ai:claude
fn ensure_store_tracked_runtime_gitignore(store_worktree: &std::path::Path) -> Result<bool> {
    use std::io::Write;
    let gitignore_path = store_worktree.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
    // Already guarded? `.aida/node.toml` is the load-bearing pattern (the one
    // that conflicts on every cross-clone rebase).
    if existing.lines().any(|l| l.trim() == ".aida/node.toml") {
        return Ok(false);
    }
    let block =
        "\n# Per-clone runtime state — must never be tracked on the orphan aida-store branch\n\
         .aida/node.toml\n\
         .aida/dispenser.toml\n\
         .aida/*.lock\n\
         .aida/cache.db\n\
         .aida/cache.db-journal\n\
         .aida/cache.db-shm\n\
         .aida/cache.db-wal\n";
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore_path)
        .with_context(|| format!("opening {}", gitignore_path.display()))?;
    f.write_all(block.as_bytes())
        .with_context(|| format!("appending to {}", gitignore_path.display()))?;
    Ok(true)
}

// TASK-570: heal an orphan queue entry by routing through the same
// `storage.queue_remove` primitive that `aida queue prune --orphaned`
// (TASK-537) uses. Idempotent — a re-run after the entry is already gone
// reports `skipped`. trace:TASK-570 | ai:claude
fn heal_doctor_orphan_queue_entry(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let user_id = current_user_id(None);
    let storage = Storage::new(project_root.join(".aida-store"));
    let entry_uuid: uuid::Uuid = finding
        .id
        .parse()
        .with_context(|| format!("parsing queue entry id {}", finding.id))?;
    let entries = storage.queue_list(&user_id, /* include_completed */ false)?;
    if !entries.iter().any(|e| e.requirement_id == entry_uuid) {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: "orphan queue entry already removed".to_string(),
            status: "skipped".to_string(),
            detail: None,
        });
    }
    storage.queue_remove(&user_id, &entry_uuid)?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "removed orphan queue entry".to_string(),
        status: "healed".to_string(),
        detail: None,
    })
}

/// Remove a cache lock-info file only when its recorded owner is provably dead
/// (PID gone, or PID reused by a different process). Re-verified at heal time
/// with a compare-and-delete, so a finding that went stale between detection
/// and heal (or a live owner past its expected duration) is never deleted.
// trace:TASK-1484 | ai:claude
fn heal_doctor_stale_lock(finding: &DoctorFinding) -> Result<DoctorHealResult> {
    let path = std::path::Path::new(&finding.id);
    let outcome = aida_core::reclaim_dead_lock_info(path)?;
    let (status, detail) = match outcome {
        aida_core::LockInfoReclaim::Removed { .. } => ("healed", None),
        aida_core::LockInfoReclaim::Absent => (
            "skipped",
            Some("lock-info file was already gone".to_string()),
        ),
        aida_core::LockInfoReclaim::Kept(Some(owner)) if owner.presumed_alive() => (
            "skipped",
            Some("lock owner is still alive (or cannot be verified); left in place".to_string()),
        ),
        aida_core::LockInfoReclaim::Kept(_) => (
            "skipped",
            Some("lock-info changed or could not be parsed; left in place".to_string()),
        ),
    };
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "removed stale cache lock-info file".to_string(),
        status: status.to_string(),
        detail,
    })
}

fn heal_doctor_orphan_branch(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["branch", "-D", &finding.id])
        .status()
        .with_context(|| format!("deleting branch {}", finding.id))?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "deleted orphan branch".to_string(),
        status: if status.success() { "healed" } else { "failed" }.to_string(),
        detail: None,
    })
}

#[cfg(test)]
mod story_462_doctor_tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn doctor_default_flags_parse_without_subcommand() {
        let cli = Cli::try_parse_from([
            "aida",
            "doctor",
            "--heal",
            "--yes",
            "--category",
            "stale-leases",
            "--json",
        ])
        .unwrap();
        let Command::Doctor {
            heal,
            yes,
            category,
            json,
            force,
            all,
            since,
            fix_sandbox,
            cmd,
            ..
        } = cli.command
        else {
            panic!("expected doctor command");
        };
        assert!(heal);
        assert!(yes);
        assert_eq!(category.as_deref(), Some("stale-leases"));
        assert!(json);
        assert!(!force);
        assert!(!all);
        assert!(since.is_none());
        assert!(!fix_sandbox);
        assert!(cmd.is_none());
    }

    #[test]
    fn normalize_doctor_category_accepts_dead_agents() {
        // STORY-496
        for alias in ["dead-agents", "dead-agent", "stale-agents", "agents"] {
            assert_eq!(normalize_doctor_category(alias).unwrap(), "dead-agents");
        }
    }

    // ── TASK-717: stale-remote-branch classification ──

    #[test]
    fn normalize_doctor_category_accepts_stale_remote_branches() {
        // TASK-717
        for alias in [
            "stale-remote-branches",
            "stale-remote-branch",
            "remote-branch",
            "remote-branches",
            "remote-branch-prune",
        ] {
            assert_eq!(
                normalize_doctor_category(alias).unwrap(),
                "stale-remote-branches"
            );
        }
    }

    #[test]
    fn normalize_doctor_category_accepts_merged_agent_worktrees() {
        // TASK-878
        for alias in [
            "merged-agent-worktrees",
            "merged-agent-worktree",
            "agent-worktree",
            "agent-worktrees",
            "worktree-gc",
            "agent-worktree-gc",
        ] {
            assert_eq!(
                normalize_doctor_category(alias).unwrap(),
                "merged-agent-worktrees"
            );
        }
    }

    // BUG-1535: a merge-gate agreed_id equal to another object's native
    // spec_id is reported under `id-collisions`, naming BOTH objects, and is
    // never marked safe to auto-heal. trace:BUG-1535 | ai:claude
    #[test]
    fn id_collisions_finding_names_both_objects() {
        use aida_core::models::{Requirement, RequirementsStore};
        let mut fixture = Requirement::new("[test] fixture".into(), "d".into());
        fixture.spec_id = Some("BUG-34".into());
        let fixture_uuid = fixture.id;
        let mut real = Requirement::new("real spec".into(), "d".into());
        real.spec_id = Some("BUG-2-081".into());
        real.agreed_id = Some("BUG-34".into());
        let mut clean = Requirement::new("clean".into(), "d".into());
        clean.spec_id = Some("BUG-35".into());
        clean.agreed_id = Some("BUG-35".into());
        let mut store = RequirementsStore::new();
        store.requirements = vec![real, fixture, clean];

        let findings = id_collision_findings(&store);
        // One collision finding + the unique-id count check.
        assert_eq!(findings.len(), 2, "{findings:?}");
        let count = findings.iter().find(|f| f.id == "store-count").unwrap();
        assert!(count.summary.contains("3 objects") && count.summary.contains("only 2 unique"));
        let f = findings.iter().find(|f| f.id == "BUG-34").unwrap();
        assert_eq!(f.category, "id-collisions");
        assert_eq!(f.id, "BUG-34");
        assert!(!f.safe_heal);
        assert!(f.summary.contains("[test] fixture"), "{}", f.summary);
        assert!(f.summary.contains("BUG-2-081"), "{}", f.summary);
        // The native owner's unambiguous handle is its uuid.
        assert!(
            f.summary.contains(&fixture_uuid.to_string()),
            "{}",
            f.summary
        );
        for alias in ["id-collisions", "ambiguous-ids", "duplicate_ids"] {
            assert_eq!(normalize_doctor_category(alias).unwrap(), "id-collisions");
        }
    }

    #[test]
    fn normalize_doctor_category_accepts_ci() {
        // trace:STORY-1043 | ai:codex
        for alias in ["ci", "cross-platform", "cross-platform-ci", "nightly-red"] {
            assert_eq!(normalize_doctor_category(alias).unwrap(), "ci");
        }
    }

    fn remote_facts() -> RemoteBranchFacts {
        RemoteBranchFacts {
            protected: false,
            has_open_pr: false,
            ancestor_of_main: false,
            spec_referenced_on_main: false,
            spec_terminal: false,
            unique_unmerged_commits: 0,
        }
    }

    #[test]
    fn classify_squash_merged_remote_branch_is_safe_to_delete() {
        // TASK-717: squash-merged → origin/main references the spec, branch tip
        // has a different SHA (so NOT an ancestor) but zero unique commits.
        let facts = RemoteBranchFacts {
            spec_referenced_on_main: true,
            ..remote_facts()
        };
        assert!(matches!(
            classify_stale_remote_branch(&facts),
            RemoteBranchVerdict::SafeToDelete(_)
        ));
    }

    #[test]
    fn classify_ancestor_of_main_remote_branch_is_safe_to_delete() {
        // TASK-717: fast-forward / non-squash merge → HEAD is an ancestor.
        let facts = RemoteBranchFacts {
            ancestor_of_main: true,
            ..remote_facts()
        };
        assert!(matches!(
            classify_stale_remote_branch(&facts),
            RemoteBranchVerdict::SafeToDelete(_)
        ));
    }

    #[test]
    fn classify_terminal_spec_remote_branch_is_safe_to_delete() {
        // TASK-717: spec is Completed/Rejected with no unique commits.
        let facts = RemoteBranchFacts {
            spec_terminal: true,
            ..remote_facts()
        };
        assert!(matches!(
            classify_stale_remote_branch(&facts),
            RemoteBranchVerdict::SafeToDelete(_)
        ));
    }

    #[test]
    fn classify_unique_unmerged_remote_branch_is_kept() {
        // TASK-717: squash-merged BUT carries genuinely-unique unmerged commits
        // (the spock-dev migration-guide case) → KEEP, never delete.
        let facts = RemoteBranchFacts {
            spec_referenced_on_main: true,
            unique_unmerged_commits: 3,
            ..remote_facts()
        };
        assert!(matches!(
            classify_stale_remote_branch(&facts),
            RemoteBranchVerdict::Keep(_)
        ));
    }

    #[test]
    fn classify_no_merge_signal_remote_branch_is_kept() {
        // TASK-717: no merged/terminal signal at all → never delete; flag.
        let facts = remote_facts();
        assert!(matches!(
            classify_stale_remote_branch(&facts),
            RemoteBranchVerdict::Keep(_)
        ));
    }

    #[test]
    fn classify_open_pr_remote_branch_is_excluded() {
        // TASK-717: an open-PR branch is excluded even if it looks merged.
        let facts = RemoteBranchFacts {
            has_open_pr: true,
            spec_referenced_on_main: true,
            ..remote_facts()
        };
        assert!(matches!(
            classify_stale_remote_branch(&facts),
            RemoteBranchVerdict::Excluded(_)
        ));
    }

    #[test]
    fn classify_protected_remote_branch_is_excluded() {
        // TASK-717: protected refs (main/master/aida-store) are excluded first,
        // even when every "merged" signal is set.
        let facts = RemoteBranchFacts {
            protected: true,
            ancestor_of_main: true,
            spec_referenced_on_main: true,
            spec_terminal: true,
            ..remote_facts()
        };
        assert!(matches!(
            classify_stale_remote_branch(&facts),
            RemoteBranchVerdict::Excluded(_)
        ));
    }

    fn agent_wt_facts() -> AgentWorktreeFacts {
        AgentWorktreeFacts {
            dirty: false,
            ancestor_of_main: false,
            pr_merged: false,
            unique_unmerged_commits: 0,
            content_fully_landed: false,
        }
    }

    #[test]
    fn classify_ancestor_of_main_agent_worktree_is_removable() {
        // TASK-878: fast-forward / non-squash merge → branch is an ancestor of
        // origin/main, clean, zero unique commits → safe to GC.
        let facts = AgentWorktreeFacts {
            ancestor_of_main: true,
            ..agent_wt_facts()
        };
        assert!(matches!(
            classify_agent_worktree(&facts),
            AgentWorktreeVerdict::Removable(_)
        ));
    }

    #[test]
    fn classify_squash_merged_agent_worktree_is_removable() {
        // TASK-878: squash-merged → branch tip differs (NOT ancestor) but the
        // forge reports a merged PR and there are zero unique commits → removable.
        let facts = AgentWorktreeFacts {
            pr_merged: true,
            ..agent_wt_facts()
        };
        assert!(matches!(
            classify_agent_worktree(&facts),
            AgentWorktreeVerdict::Removable(_)
        ));
    }

    #[test]
    fn classify_unmerged_commits_agent_worktree_is_kept() {
        // TASK-878: PR merged BUT the branch carries genuinely-unique unmerged
        // commits (extra work added after the squash merge) → KEEP, never remove.
        let facts = AgentWorktreeFacts {
            pr_merged: true,
            unique_unmerged_commits: 2,
            ..agent_wt_facts()
        };
        assert!(matches!(
            classify_agent_worktree(&facts),
            AgentWorktreeVerdict::Keep(_)
        ));
    }

    #[test]
    fn classify_content_verified_squash_merge_is_removable() {
        // BUG-1287: PR merged, ancestry reports a positive commit count (the
        // squash-merge case can NEVER clear this by ancestry alone — the
        // branch's original commits keep a different SHA from the squash
        // commit forever) BUT a content-level check proves every file the
        // branch touched already matches origin/main byte-for-byte → the
        // count is an ancestry artifact, not real unshipped work → Removable.
        let facts = AgentWorktreeFacts {
            pr_merged: true,
            unique_unmerged_commits: 1,
            content_fully_landed: true,
            ..agent_wt_facts()
        };
        assert!(matches!(
            classify_agent_worktree(&facts),
            AgentWorktreeVerdict::Removable(_)
        ));
    }

    #[test]
    fn classify_partially_landed_squash_merge_is_kept() {
        // BUG-1287 companion: content_fully_landed stays false when even ONE
        // of the branch's touched files still differs from main — e.g. later
        // commits added after the PR squash-merged that never shipped. The
        // positive commit count keeps this KEPT, exactly like the plain
        // unmerged-commits case above.
        let facts = AgentWorktreeFacts {
            pr_merged: true,
            unique_unmerged_commits: 4,
            content_fully_landed: false,
            ..agent_wt_facts()
        };
        assert!(matches!(
            classify_agent_worktree(&facts),
            AgentWorktreeVerdict::Keep(_)
        ));
    }

    /// BUG-1287 fixture: a real git repo with two squash-merged branches —
    /// one where the squash-merge shipped ALL of the branch's content (the
    /// false-positive case ancestry alone can never clear), and one where a
    /// later commit was added to the branch AFTER the squash landed and never
    /// shipped (genuinely unique, must stay kept). Asserts
    /// `branch_content_fully_landed` tells them apart — the reapable one from
    /// the not-yet-reapable one — by durable patch identity, not by ancestry
    /// or a point-in-time comparison of current trees.
    // trace:BUG-1287 | ai:claude
    // trace:BUG-1424 | ai:codex
    #[test]
    fn branch_content_fully_landed_distinguishes_shipped_from_unshipped_squash_branches() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();

        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };

        std::process::Command::new("git")
            .arg("init")
            .arg(&root)
            .output()
            .unwrap();
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@example.com"]);
        std::fs::write(root.join("README.md"), "base\n").unwrap();
        run(&["add", "README.md"]);
        run(&["commit", "-m", "init"]);
        run(&["branch", "-M", "main"]);

        // Branch A: two branch commits are combined into one squash commit on
        // main. Neither source commit is patch-equivalent to the cumulative
        // squash, but the branch's merge-base-to-tip patch is.
        run(&["checkout", "-b", "shipped-work"]);
        std::fs::write(root.join("a.txt"), "shipped content\n").unwrap();
        run(&["add", "a.txt"]);
        run(&["commit", "-m", "add a.txt"]);
        std::fs::write(root.join("a2.txt"), "second shipped change\n").unwrap();
        run(&["add", "a2.txt"]);
        run(&["commit", "-m", "add a2.txt"]);
        run(&["checkout", "main"]);
        // Simulate a forge squash-merge: both files land in one brand-new
        // commit, unrelated by ancestry to either branch commit.
        std::fs::write(root.join("a.txt"), "shipped content\n").unwrap();
        std::fs::write(root.join("a2.txt"), "second shipped change\n").unwrap();
        run(&["add", "a.txt", "a2.txt"]);
        run(&["commit", "-m", "squash-merge shipped-work (#1)"]);

        // Branch B: its FIRST commit is squash-merged the same way, but a
        // SECOND commit is added afterward that never ships — genuinely
        // unique, unmerged work that must never be reported as landed.
        run(&["checkout", "-b", "partial-work"]);
        run(&["reset", "--hard", "main~1"]); // back to pre-squash main tip
        std::fs::write(root.join("b.txt"), "partial content\n").unwrap();
        run(&["add", "b.txt"]);
        run(&["commit", "-m", "add b.txt"]);
        std::fs::write(root.join("b2.txt"), "never shipped\n").unwrap();
        run(&["add", "b2.txt"]);
        run(&["commit", "-m", "follow-up never merged"]);
        run(&["checkout", "main"]);
        std::fs::write(root.join("b.txt"), "partial content\n").unwrap();
        run(&["add", "b.txt"]);
        run(&["commit", "-m", "squash-merge partial-work (#2)"]);

        // Move the world after both squash merges. In particular, change a
        // file shipped-work touched: the old tree comparison now says the
        // branch differs, while its patch remains durably present in main's
        // bounded history.
        std::fs::write(root.join("a.txt"), "shipped content\nlater main edit\n").unwrap();
        run(&["add", "a.txt"]);
        run(&["commit", "-m", "advance main and revisit overlapping file"]);

        // shipped-work: ancestry can never clear this (different SHA forever),
        // and the current trees no longer match, but the landed patch is still
        // present in main's bounded history.
        let count_out = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-list", "--count", "main..shipped-work"])
            .output()
            .unwrap();
        let ancestry_count: u32 = String::from_utf8_lossy(&count_out.stdout)
            .trim()
            .parse()
            .unwrap();
        assert_eq!(
            ancestry_count, 2,
            "ancestry must still see both squash-merged branch commits as unique"
        );
        let per_commit_out = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args([
                "rev-list",
                "--cherry-pick",
                "--right-only",
                "main...shipped-work",
            ])
            .output()
            .unwrap();
        assert!(per_commit_out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&per_commit_out.stdout)
                .lines()
                .count(),
            2,
            "per-commit patch matching must miss both commits combined by the squash"
        );
        assert!(
            branch_content_fully_landed(&root, "main", "shipped-work"),
            "squash-merged branch must remain fully landed after main changes an overlapping file"
        );

        // partial-work: one file matches main, but b2.txt never shipped — the
        // content check must stay conservative and report NOT fully landed.
        assert!(
            !branch_content_fully_landed(&root, "main", "partial-work"),
            "a branch carrying a genuinely-unshipped file must never be reported fully landed"
        );
    }

    // BUG-1288: `branch_content_fully_landed`'s squash-merge fallback used to
    // spawn a `git show | git patch-id` subprocess PAIR per commit between a
    // branch's merge-base and the default branch. On the aida repo itself,
    // one call with a 2,496-commit range measured 3m41s of wall clock — the
    // dominant cause of `aida awaiting --json` / `aida status --full`
    // blocking for 70-90s. This fixture reproduces the same shape at a
    // CI-affordable scale (500+ commits) and pins two things at once: the
    // deep-history branch must not block the caller (the new
    // `MAX_SQUASH_FALLBACK_COMMITS` bound bails out instead of walking the
    // whole range), and bailing out must stay conservative — a genuinely
    // unshipped branch still reads `false` ("not confirmed landed"), never a
    // false `true`, so real unshipped work can never be hidden by this bound
    // (PRIN-5 / BUG-1288 acceptance #6). The 30s budget is deliberately
    // generous: this pins "does not regress back to unbounded", not a tight
    // perf target — this bug's own history (the candidate population roughly
    // 8x'd between when the spec was filed and when this fix landed) is the
    // reason a tight wall-clock assertion would be the wrong thing to pin in
    // CI. trace:BUG-1288 | ai:claude
    #[test]
    fn branch_content_fully_landed_bails_out_on_a_deep_history_range() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();

        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };

        std::process::Command::new("git")
            .arg("init")
            .arg(&root)
            .output()
            .unwrap();
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@example.com"]);
        std::fs::write(root.join("README.md"), "base\n").unwrap();
        run(&["add", "README.md"]);
        run(&["commit", "-m", "init"]);
        run(&["branch", "-M", "main"]);

        // Branch off right away — this commit is the merge-base the filler
        // history below piles up past.
        run(&["checkout", "-b", "deep-history-work"]);
        std::fs::write(root.join("never-shipped.txt"), "genuinely unshipped\n").unwrap();
        run(&["add", "never-shipped.txt"]);
        run(&["commit", "-m", "add never-shipped.txt"]);
        run(&["checkout", "main"]);

        // Push main past MAX_SQUASH_FALLBACK_COMMITS (500) commits since the
        // branch's merge-base — the shape that took 3m41s pre-fix on real
        // history.
        for i in 0..520 {
            run(&["commit", "--allow-empty", "-m", &format!("filler {i}")]);
        }

        let merge_base_out = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["merge-base", "main", "deep-history-work"])
            .output()
            .unwrap();
        let merge_base = String::from_utf8_lossy(&merge_base_out.stdout)
            .trim()
            .to_string();
        let count_out = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-list", "--count", &format!("{merge_base}..main")])
            .output()
            .unwrap();
        let default_range_count: u32 = String::from_utf8_lossy(&count_out.stdout)
            .trim()
            .parse()
            .unwrap();
        assert!(
            default_range_count > 500,
            "fixture must exceed MAX_SQUASH_FALLBACK_COMMITS to exercise the bound, got {default_range_count}"
        );

        // Warm the fixture (git's loose-object access, this process's page
        // cache, …) before the timed call, so the assertion below measures
        // the bounded algorithm's own cost, not first-touch overhead.
        let _ = branch_content_fully_landed(&root, "main", "deep-history-work");

        let budget = std::time::Duration::from_secs(30);
        let started = std::time::Instant::now();
        let landed = branch_content_fully_landed(&root, "main", "deep-history-work");
        let elapsed = started.elapsed();

        assert!(
            elapsed < budget,
            "deep-history squash-fallback took {elapsed:?}, expected well under the \
             {budget:?} regression budget (pre-fix this shape measured 3m41s on real history)"
        );
        assert!(
            !landed,
            "a genuinely-unshipped branch must stay reported as NOT landed even when the \
             expensive proof is skipped for being too large — never silently true"
        );
    }

    #[test]
    fn classify_dirty_agent_worktree_is_kept_even_when_merged() {
        // TASK-878: clean != no work, but DIRTY is unambiguously work — a
        // worktree with uncommitted changes is KEPT even when its branch merged.
        let facts = AgentWorktreeFacts {
            dirty: true,
            ancestor_of_main: true,
            pr_merged: true,
            ..agent_wt_facts()
        };
        assert!(matches!(
            classify_agent_worktree(&facts),
            AgentWorktreeVerdict::Keep(_)
        ));
    }

    #[test]
    fn classify_no_merge_signal_agent_worktree_is_kept() {
        // TASK-878: no merge signal at all (unmerged work in flight) → never
        // remove; flag for the operator.
        let facts = AgentWorktreeFacts {
            unique_unmerged_commits: 5,
            ..agent_wt_facts()
        };
        assert!(matches!(
            classify_agent_worktree(&facts),
            AgentWorktreeVerdict::Keep(_)
        ));
    }

    // ----- BUG-614: conservative worktree GC predicate -----------------------

    /// The all-safe baseline: agent-managed, merged-or-gone, clean, unlocked,
    /// no active lease → the only combination the GC will remove.
    fn gc_eligible_facts() -> WorktreeGcFacts {
        WorktreeGcFacts {
            is_agent_managed: true,
            merged_or_gone: true,
            dirty: false,
            locked: false,
            has_active_lease: false,
        }
    }

    #[test]
    fn gc_merged_clean_unlocked_no_lease_is_eligible() {
        // BUG-614: all four safety gates pass → Eligible.
        assert_eq!(
            classify_worktree_gc(&gc_eligible_facts()),
            WorktreeGcVerdict::Eligible
        );
    }

    #[test]
    fn gc_dirty_worktree_is_preserved() {
        // BUG-614: uncommitted work is unambiguously "has work" — preserved even
        // when its branch merged.
        let facts = WorktreeGcFacts {
            dirty: true,
            ..gc_eligible_facts()
        };
        assert!(matches!(
            classify_worktree_gc(&facts),
            WorktreeGcVerdict::Preserve(_)
        ));
    }

    #[test]
    fn gc_locked_worktree_is_preserved() {
        // BUG-614: a locked worktree is operator-protected — never removed.
        let facts = WorktreeGcFacts {
            locked: true,
            ..gc_eligible_facts()
        };
        assert!(matches!(
            classify_worktree_gc(&facts),
            WorktreeGcVerdict::Preserve(_)
        ));
    }

    #[test]
    fn gc_unmerged_branch_worktree_is_preserved() {
        // BUG-614: a branch that is NOT merged into the default branch carries
        // unmerged work — preserved.
        let facts = WorktreeGcFacts {
            merged_or_gone: false,
            ..gc_eligible_facts()
        };
        assert!(matches!(
            classify_worktree_gc(&facts),
            WorktreeGcVerdict::Preserve(_)
        ));
    }

    #[test]
    fn gc_active_lease_worktree_is_preserved() {
        // BUG-614: a live process / active session-lease pins the worktree —
        // never removed, even merged + clean.
        let facts = WorktreeGcFacts {
            has_active_lease: true,
            ..gc_eligible_facts()
        };
        assert!(matches!(
            classify_worktree_gc(&facts),
            WorktreeGcVerdict::Preserve(_)
        ));
    }

    #[test]
    fn gc_non_agent_worktree_is_preserved() {
        // BUG-614: the GC only ever touches agent-managed worktrees; a normal
        // work-branch worktree is preserved regardless of the other facts.
        let facts = WorktreeGcFacts {
            is_agent_managed: false,
            ..gc_eligible_facts()
        };
        assert!(matches!(
            classify_worktree_gc(&facts),
            WorktreeGcVerdict::Preserve(_)
        ));
    }

    #[test]
    fn gc_dirty_takes_priority_in_reason_over_unmerged() {
        // BUG-614: when several gates fail, the costliest objection (dirty) is
        // reported — losing uncommitted work is the worst outcome.
        let facts = WorktreeGcFacts {
            dirty: true,
            locked: true,
            merged_or_gone: false,
            has_active_lease: true,
            ..gc_eligible_facts()
        };
        match classify_worktree_gc(&facts) {
            WorktreeGcVerdict::Preserve(reason) => assert!(
                reason.contains("uncommitted"),
                "dirty objection must win, got: {reason}"
            ),
            other => panic!("expected Preserve, got {other:?}"),
        }
    }

    #[test]
    fn gc_worktree_is_active_matches_self_and_descendants() {
        // BUG-614: an active path equal to the worktree OR beneath it pins it.
        let tmp = tempfile::TempDir::new().unwrap();
        let wt = tmp.path().join("agent-abc");
        std::fs::create_dir_all(wt.join("nested")).unwrap();
        let wt_canon = wt.canonicalize().unwrap();
        let nested_canon = wt.join("nested").canonicalize().unwrap();

        // Exact match.
        let mut active = HashSet::new();
        active.insert(wt_canon.clone());
        assert!(worktree_is_active(&wt, &active));

        // Descendant (a live claude one level down) still pins it.
        let mut active = HashSet::new();
        active.insert(nested_canon);
        assert!(worktree_is_active(&wt, &active));

        // Unrelated path does not pin it.
        let other = tmp.path().join("agent-other");
        std::fs::create_dir_all(&other).unwrap();
        let mut active = HashSet::new();
        active.insert(other.canonicalize().unwrap());
        assert!(!worktree_is_active(&wt, &active));
    }

    #[test]
    fn is_agent_managed_worktree_matches_path_and_branch() {
        // TASK-878: scoped to the Agent-tool isolation worktrees by EITHER the
        // path segment or the branch convention.
        use std::path::Path;
        assert!(is_agent_managed_worktree(
            Path::new("/repo/.claude/worktrees/agent-abc123"),
            Some("worktree-agent-abc123"),
        ));
        // Path-only match (branch missing / detached).
        assert!(is_agent_managed_worktree(
            Path::new("/repo/.claude/worktrees/agent-deadbeef"),
            None,
        ));
        // Branch-only match (oddly-pathed but conventional branch).
        assert!(is_agent_managed_worktree(
            Path::new("/tmp/scratch"),
            Some("worktree-agent-9f9f"),
        ));
        // A normal work-branch worktree is NOT in scope.
        assert!(!is_agent_managed_worktree(
            Path::new("/repo/wt/task-281"),
            Some("task-281-foo"),
        ));
    }

    #[test]
    fn merged_agent_worktree_heal_is_gated_in_autonomous_context() {
        // TASK-878 + STORY-666: a merged-agent-worktree finding is destructive
        // (safe_heal=false), so the per-category disposition GATES it in an
        // autonomous context even with --force --yes, and PROCEEDS interactively.
        // matches the finding's classification (destructive → never safe_heal)
        let safe_heal = false;
        // Unattended (autonomous) → fail-closed refusal.
        assert_eq!(
            doctor_heal_disposition(safe_heal, /*force*/ true, /*yes*/ true, /*auto*/ true),
            HealDisposition::GateAutonomous,
        );
        // Interactive TTY sign-off → proceeds.
        assert_eq!(
            doctor_heal_disposition(safe_heal, /*force*/ true, /*yes*/ true, /*auto*/ false),
            HealDisposition::Proceed,
        );
        // Without --force --yes it's the pre-existing manual-decision skip.
        assert_eq!(
            doctor_heal_disposition(safe_heal, /*force*/ false, /*yes*/ false, /*auto*/ false),
            HealDisposition::SkipNeedsForce,
        );
    }

    #[test]
    fn spec_id_from_work_branch_derives_spec_id() {
        // TASK-717
        assert_eq!(
            spec_id_from_work_branch("task-281-foo"),
            Some("TASK-281".to_string())
        );
        assert_eq!(
            spec_id_from_work_branch("task-1-127"),
            Some("TASK-1-127".to_string())
        );
        assert_eq!(
            spec_id_from_work_branch("task-1-127-fix"),
            Some("TASK-1-127".to_string())
        );
        assert_eq!(
            spec_id_from_work_branch("bug-100"),
            Some("BUG-100".to_string())
        );
        assert_eq!(
            spec_id_from_work_branch("STORY-86"),
            Some("STORY-86".to_string())
        );
        // Non-work branches yield nothing.
        assert_eq!(spec_id_from_work_branch("spock-dev"), None);
        assert_eq!(spec_id_from_work_branch("pr-271"), None);
    }

    #[test]
    fn doctor_detects_and_reaps_dead_agent_registry_entry() {
        // STORY-496: a registry entry whose pid is dead is reported under
        // `dead-agents` and reaped by the heal.
        let tmp = tempfile::TempDir::new().unwrap();
        let project = tmp.path().to_path_buf();
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        let dead_pid = 4_294_967_294u32; // far above any real pid → never alive
        assert!(!process_probe::pid_is_alive(dead_pid));
        agent_registry::register_spawned_agent(
            &project,
            "claude",
            dead_pid,
            None,
            None,
            project.clone(),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Detect.
        let store = aida_core::models::RequirementsStore::new();
        let findings = collect_doctor_findings(&project, &store, Some("dead-agents")).unwrap();
        assert_eq!(findings.len(), 1, "one dead-agent finding");
        assert_eq!(findings[0].category, "dead-agents");
        assert_eq!(findings[0].id, format!("claude#{dead_pid}"));
        assert!(findings[0].safe_heal);

        // Heal.
        let result = heal_doctor_dead_agent(&project, &findings[0]).unwrap();
        assert_eq!(result.status, "healed");

        // Reaped → no longer reported.
        let after = collect_doctor_findings(&project, &store, Some("dead-agents")).unwrap();
        assert!(after.is_empty(), "reaped entry must not reappear");
    }

    #[test]
    fn heal_doctor_lease_is_idempotent_when_lease_already_gone() {
        // BUG-471: a stale-lease finding whose lease no longer exists (ended
        // earlier in the same heal run, or by a concurrent session) must heal to
        // a no-op skip — NOT an error that aborts the whole heal.
        let tmp = tempfile::TempDir::new().unwrap();
        let project = tmp.path().to_path_buf();
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        let finding = DoctorFinding {
            category: "stale-leases".to_string(),
            id: "deadbeef0000".to_string(),
            summary: "stale lease".to_string(),
            action: "end lease deadbeef0000".to_string(),
            safe_heal: true,
        };
        let result = heal_doctor_lease(&project, &finding).expect("absent lease must not error");
        assert_eq!(result.status, "skipped");
        assert!(result
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("already ended"));
    }

    #[test]
    fn doctor_heal_reaps_stale_lease_before_spec_status_bump() {
        // BUG-1136: a single all-category heal must converge. The original
        // findings can contain both "Approved + lease => bump" and
        // "stale lease => reap"; after reaping, the stale status action is no
        // longer valid and must not leave the spec In Progress with no lease.
        let tmp = tempfile::TempDir::new().unwrap();
        let project = tmp.path();
        std::fs::create_dir_all(project.join(".aida").join("sessions")).unwrap();
        std::fs::create_dir_all(project.join(".aida-store")).unwrap();

        let storage = Storage::new(project.join(".aida-store"));
        let mut req = Requirement::new("lease/status drift".into(), String::new());
        req.spec_id = Some("TASK-1207".into());
        req.status = RequirementStatus::Approved;
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![req];
        storage.save(&store).unwrap();

        std::fs::write(
            project
                .join(".aida")
                .join("sessions")
                .join("deadbeef0000.toml"),
            r#"
id = "deadbeef0000"
scope = "TASK-1207"
slug = "task-1207"
owner = "tester"
worktree_path = ""
branch = "bug-1136"
started_at = "2026-09-13T00:00:00Z"
hostname = "localhost"
"#,
        )
        .unwrap();

        let stale_lease = DoctorFinding {
            category: "stale-leases".to_string(),
            id: "deadbeef0000".to_string(),
            summary: "deadbeef0000 owns TASK-1207".to_string(),
            action: "save patch if dirty, then end lease deadbeef0000".to_string(),
            safe_heal: true,
        };
        let stale_status_bump = DoctorFinding {
            category: "spec-status-drift".to_string(),
            id: "TASK-1207".to_string(),
            summary: "TASK-1207 is Approved but lease deadbeef0000 is active".to_string(),
            action: "bump spec to In Progress".to_string(),
            safe_heal: true,
        };

        let results = heal_doctor_findings(
            project,
            &[stale_status_bump, stale_lease],
            &DoctorRunOptions {
                heal: true,
                yes: true,
                category: None,
                json: false,
                force: false,
                all: false,
                since: None,
                fail_on_findings: false,
            },
        )
        .unwrap();

        assert_eq!(results[0].category, "stale-leases");
        assert_eq!(results[1].category, "spec-status-drift");
        assert_eq!(results[1].status, "skipped");
        assert!(!project
            .join(".aida")
            .join("sessions")
            .join("deadbeef0000.toml")
            .exists());
        let reloaded = storage.load().unwrap();
        assert_eq!(reloaded.requirements[0].status, RequirementStatus::Approved);
    }

    #[test]
    fn doctor_check_and_heal_subcommands_parse() {
        let check =
            Cli::try_parse_from(["aida", "doctor", "check", "OBE-briefs", "--json", "--all"])
                .unwrap();
        let Command::Doctor {
            cmd:
                Some(cli::DoctorCommand::Check {
                    category,
                    all,
                    json,
                    ..
                }),
            ..
        } = check.command
        else {
            panic!("expected doctor check");
        };
        assert_eq!(category, "OBE-briefs");
        assert!(all);
        assert!(json);

        let heal = Cli::try_parse_from([
            "aida",
            "doctor",
            "heal",
            "orphan-branches",
            "--yes",
            "--force",
            "--all",
        ])
        .unwrap();
        let Command::Doctor {
            cmd:
                Some(cli::DoctorCommand::Heal {
                    category,
                    yes,
                    force,
                    all,
                    json,
                }),
            ..
        } = heal.command
        else {
            panic!("expected doctor heal");
        };
        assert_eq!(category, "orphan-branches");
        assert!(yes);
        assert!(force);
        assert!(all);
        assert!(!json);
    }

    // trace:TASK-1124 — codex_prompts_drift flags a stale deployed prompt,
    // ignores a matching one, and never nags about a prompt not deployed.
    #[test]
    fn codex_prompts_drift_flags_only_stale_deployed_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let expected = aida_core::scaffolding::codex_prompts::expected_codex_prompts();
        assert!(expected.len() >= 2, "need a couple of prompts to test");
        // Deploy the first prompt correctly (matches source).
        let (fresh_name, fresh_body) = &expected[0];
        std::fs::write(
            dir.path().join(format!("{fresh_name}.md")),
            fresh_body.replace('\n', "\r\n"),
        )
        .unwrap();
        // Deploy the second prompt STALE (content differs).
        let (stale_name, _) = &expected[1];
        std::fs::write(
            dir.path().join(format!("{stale_name}.md")),
            "stale content\n",
        )
        .unwrap();
        // Every other prompt is never deployed (missing) — must NOT be flagged.

        let drifted = codex_prompts_drift(dir.path());
        assert!(
            drifted.contains(stale_name),
            "stale prompt must be flagged: {drifted:?}"
        );
        assert!(
            !drifted.contains(fresh_name),
            "a matching prompt must NOT be flagged: {drifted:?}"
        );
        assert_eq!(
            drifted.len(),
            1,
            "only the one stale prompt should be flagged (missing = opt-out): {drifted:?}"
        );
    }

    // trace:BUG-1095 — Codex CLI 0.142 ignores ~/.codex/prompts as a custom
    // slash-command surface, so doctor must catch generated prompt dirs that
    // users might reasonably expect to expose /aida-* commands.
    #[test]
    fn codex_prompt_dir_warning_is_version_gated() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("aida-capture.md"), "prompt body\n").unwrap();

        assert!(
            codex_ignores_prompt_dir_finding_for_version(dir.path(), (0, 141, 9)).is_none(),
            "older Codex versions stay quiet because the field report only verifies 0.142+"
        );
        let finding =
            codex_ignores_prompt_dir_finding_for_version(dir.path(), (0, 142, 0)).unwrap();
        assert_eq!(finding.id, "scaffold-drift/codex-prompts-undiscovered");
        assert!(finding.summary.contains("0.142.0"));
        assert!(finding.summary.contains("does not discover"));
        assert!(finding.action.contains("Prune"));
        assert!(finding.action.contains(".aida-bak"));
        assert!(finding.action.contains("$aida-*"));
        assert!(finding.action.contains("/skills"));
    }

    #[test]
    fn codex_prompt_dir_warning_requires_aida_prompt_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.md"), "not an AIDA prompt\n").unwrap();
        assert!(
            codex_ignores_prompt_dir_finding_for_version(dir.path(), (0, 142, 0)).is_none(),
            "non-AIDA markdown in ~/.codex/prompts is not our scaffold state"
        );
    }

    /// A delivered-then-deleted skill is an opt-out, not missing-skill drift:
    /// doctor neither flags it nor recommends `aida scaffold upgrade`.
    // trace:TASK-1503 | ai:claude
    #[test]
    fn scaffold_drift_ignores_deleted_delivered_skill() {
        let dir = tempfile::tempdir().unwrap();
        let store = aida_core::RequirementsStore::new();
        let mut scaffolder = aida_core::scaffolding::Scaffolder::new(
            dir.path().to_path_buf(),
            ScaffoldConfig::default(),
        );
        let preview = scaffolder.preview(&store);
        scaffolder.apply(&preview).unwrap();
        std::fs::remove_dir_all(dir.path().join(".codex/skills/aida-commit")).unwrap();
        std::fs::remove_dir_all(dir.path().join(".claude/skills/aida-commit")).unwrap();

        let findings = scan_scaffold_drift(dir.path(), &store);
        assert!(
            findings
                .iter()
                .all(|f| f.id != "scaffold-drift/codex-skills-missing"),
            "{:?}",
            findings.iter().map(|f| &f.id).collect::<Vec<_>>()
        );
        let status = check_scaffold_status(
            &store,
            dir.path(),
            &ScaffoldConfig::default(),
            &dir.path().join(".aida/cache.db"),
        );
        assert!(
            status
                .missing
                .iter()
                .all(|p| !p.to_string_lossy().contains("aida-commit")),
            "{:?}",
            status.missing
        );
    }

    /// A pre-manifest (legacy) Codex pack with missing skills still gets the
    /// `aida scaffold upgrade` hint: the explicit upgrade delivers them once.
    // trace:TASK-1503 | ai:claude
    #[test]
    fn scaffold_drift_still_flags_legacy_pack_missing_skills() {
        let dir = tempfile::tempdir().unwrap();
        let store = aida_core::RequirementsStore::new();
        let req = dir.path().join(".codex/skills/aida-req");
        std::fs::create_dir_all(&req).unwrap();
        std::fs::write(req.join("SKILL.md"), "legacy\n").unwrap();
        let findings = scan_scaffold_drift(dir.path(), &store);
        let finding = findings
            .iter()
            .find(|f| f.id == "scaffold-drift/codex-skills-missing")
            .expect("legacy pack's missing skills are flagged");
        assert!(finding.action.contains("aida scaffold upgrade"));
    }

    #[test]
    fn scaffold_drift_flags_missing_codex_skills_with_exact_fix() {
        let dir = tempfile::tempdir().unwrap();
        let store = aida_core::RequirementsStore::new();

        let findings = scan_scaffold_drift(dir.path(), &store);
        let finding = findings
            .iter()
            .find(|f| f.id == "scaffold-drift/codex-skills-missing")
            .expect("missing .codex/skills should be flagged as scaffold drift");

        assert!(finding.summary.contains(".codex/skills"));
        assert!(finding.summary.contains("$aida-*"));
        assert!(finding.action.contains("aida scaffold upgrade"));
        assert!(finding.action.contains(".codex/skills/aida-*/SKILL.md"));
        assert!(finding.action.contains("$aida-capture"));
    }

    #[test]
    fn codex_prompt_dir_warning_detects_backup_cruft() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("aida-capture.md.aida-bak"), "old prompt\n").unwrap();
        let finding =
            codex_ignores_prompt_dir_finding_for_version(dir.path(), (0, 154, 0)).unwrap();
        assert_eq!(finding.id, "scaffold-drift/codex-prompts-undiscovered");
        assert!(finding.action.contains(".aida-bak"));
    }

    #[test]
    fn parse_codex_semver_accepts_common_banners() {
        assert_eq!(parse_codex_semver("codex-cli 0.142.0"), Some((0, 142, 0)));
        assert_eq!(parse_codex_semver("codex 1.2.3-beta"), Some((1, 2, 3)));
        assert_eq!(parse_codex_semver("no version here"), None);
    }

    #[test]
    fn legacy_doctor_subcommands_still_parse() {
        let cli = Cli::try_parse_from(["aida", "doctor", "fsck"]).unwrap();
        let Command::Doctor {
            cmd: Some(cli::DoctorCommand::Fsck),
            ..
        } = cli.command
        else {
            panic!("expected legacy doctor fsck");
        };
    }

    // trace:TASK-956 | ai:claude — the TASK-935 surface cuts hide verbs from
    // `--help` with clap `hide = true` but MUST leave them dispatchable. These
    // parse-tests are the bouncer: a hidden command that stops parsing would be
    // a real capability regression, not just a help-text change.
    #[test]
    fn task_956_hidden_top_level_parents_still_dispatch() {
        // punts / worker / headless are hidden from `aida --help` but still parse.
        assert!(matches!(
            Cli::try_parse_from(["aida", "punts", "analyze"])
                .unwrap()
                .command,
            Command::Punts(_)
        ));
        assert!(matches!(
            Cli::try_parse_from(["aida", "worker", "directives"])
                .unwrap()
                .command,
            Command::Worker(_)
        ));
        assert!(matches!(
            Cli::try_parse_from(["aida", "headless", "tail"])
                .unwrap()
                .command,
            Command::Headless(_)
        ));
    }

    // trace:TASK-956 | ai:claude
    #[test]
    fn task_956_hidden_subcommands_still_dispatch() {
        // doctor verify-relationships — hidden under `doctor`, still parses.
        assert!(matches!(
            Cli::try_parse_from(["aida", "doctor", "verify-relationships", "--repair"])
                .unwrap()
                .command,
            Command::Doctor {
                cmd: Some(cli::DoctorCommand::VerifyRelationships { repair: true, .. }),
                ..
            }
        ));
        // queue load — hidden under `queue`, still parses.
        assert!(matches!(
            Cli::try_parse_from(["aida", "queue", "load"])
                .unwrap()
                .command,
            Command::Queue(cli::QueueCommand::Load { .. })
        ));
        // role repair — hidden under `role`, still parses.
        assert!(matches!(
            Cli::try_parse_from(["aida", "role", "repair"])
                .unwrap()
                .command,
            Command::Role(cli::RoleCommand::Repair { .. })
        ));
    }

    // trace:TASK-956 | ai:claude — the two "merge" twins keep their own
    // distinct behavior (they are NOT collapsed into the canonical sibling's
    // logic, which would drop flags / change the output sink) but are hidden
    // from their parent's `--help`. The canonical sibling stays visible.
    #[test]
    fn task_956_merged_twins_still_dispatch_their_own_logic() {
        // changelog generate — hidden, but keeps its stdout/range surface
        // distinct from the canonical `changelog refresh`.
        assert!(matches!(
            Cli::try_parse_from(["aida", "changelog", "generate", "--since", "v0.7.0"])
                .unwrap()
                .command,
            Command::Changelog(cli::ChangelogCommand::Generate { since: Some(_), .. })
        ));
        // The canonical `changelog refresh` is unchanged and still parses.
        assert!(matches!(
            Cli::try_parse_from(["aida", "changelog", "refresh"])
                .unwrap()
                .command,
            Command::Changelog(cli::ChangelogCommand::Refresh { .. })
        ));
        // doctor fsck — hidden, but keeps its own full-suite logic distinct
        // from the canonical `doctor check` (which requires a category arg).
        assert!(matches!(
            Cli::try_parse_from(["aida", "doctor", "fsck"])
                .unwrap()
                .command,
            Command::Doctor {
                cmd: Some(cli::DoctorCommand::Fsck),
                ..
            }
        ));
    }

    #[test]
    fn doctor_category_aliases_normalize() {
        assert_eq!(normalize_doctor_category("leases").unwrap(), "stale-leases");
        assert_eq!(
            normalize_doctor_category("OBE_briefs").unwrap(),
            "OBE-briefs"
        );
        assert_eq!(normalize_doctor_category("locks").unwrap(), "stale-locks");
        assert_eq!(
            normalize_doctor_category("agent-wiring").unwrap(),
            "agents-wiring"
        );
        assert_eq!(
            normalize_doctor_category("container-gitdir").unwrap(),
            "worktree-container-gitdir"
        );
        assert!(normalize_doctor_category("not-a-category").is_err());
    }

    // BUG-1554: the user-facing "valid categories" list in the
    // unknown-category error was a hand-typed string literal that silently
    // fell behind the normalizer's alias table — at the tree this bug was
    // filed against, `external-import-bleed` and `project-manifest` both
    // dispatched and normalized correctly but were absent from the error
    // message, so a user who mistyped one of those names got an
    // authoritative-looking list that didn't contain the name they wanted.
    //
    // This asserts the two are structurally impossible to omit going
    // forward: the error message is built by mapping over
    // `DOCTOR_CATEGORY_ALIASES`, the same table `normalize_doctor_category`
    // matches against, so every canonical name is guaranteed present with
    // no second edit.
    // trace:BUG-1554 | ai:claude
    #[test]
    fn unknown_doctor_category_error_lists_every_canonical_category() {
        let err = normalize_doctor_category("not-a-real-category")
            .unwrap_err()
            .to_string();
        for (_, canonical) in DOCTOR_CATEGORY_ALIASES {
            assert!(
                err.contains(canonical),
                "error message missing canonical category `{canonical}`: {err}"
            );
        }
        // The two categories BUG-1554 found missing at HEAD 21c393f4da.
        assert!(err.contains("external-import-bleed"));
        assert!(err.contains("project-manifest"));
    }

    #[test]
    fn doctor_reports_worktree_container_gitdir() {
        // BUG-915
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(root.join(".aida-store")).unwrap();
        std::fs::write(root.join("Dockerfile"), "FROM scratch\n").unwrap();

        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        std::fs::create_dir_all(&root).unwrap();
        std::process::Command::new("git")
            .arg("init")
            .arg(&root)
            .output()
            .unwrap();
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["add", "Dockerfile"]);
        run(&["commit", "-m", "init"]);

        let worktree = tmp.path().join("repo-bug915");
        run(&[
            "worktree",
            "add",
            worktree.to_str().unwrap(),
            "-b",
            "bug915",
        ]);

        let findings = collect_doctor_findings(
            &root,
            &aida_core::RequirementsStore::default(),
            Some("worktree-container-gitdir"),
        )
        .unwrap();
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.category, "worktree-container-gitdir");
        assert!(f.summary.contains("Dockerfile"), "{:?}", f.summary);
        assert!(f.summary.contains(".git/worktrees"), "{:?}", f.summary);
        assert!(f.action.contains("mount"), "{:?}", f.action);
    }

    #[test]
    fn doctor_reports_antigravity_wiring_gaps_when_profile_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let home = tmp.path().join("home");
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_HOME", home.to_str().unwrap()),
            ("AIDA_TEST_ANTIGRAVITY_BINARY", "1"),
            ("AIDA_TEST_ANTIGRAVITY_MCP_REGISTERED", "0"),
        ]);
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[agents]\nenabled = [\"antigravity\"]\n",
        )
        .unwrap();

        let findings = collect_doctor_findings(
            root,
            &aida_core::models::RequirementsStore::new(),
            Some("agents-wiring"),
        )
        .unwrap();

        let ids: std::collections::HashSet<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(ids.contains("antigravity/agents-md"));
        assert!(ids.contains("antigravity/mcp"));
        assert!(findings.iter().any(|f| {
            f.action
                == "antigravity --add-mcp '{\"name\":\"aida\",\"command\":\"aida\",\"args\":[\"mcp-serve\"]}'"
        }));
    }

    // ---- TASK-673: completed-without-commit integrity tripwire ----

    #[test]
    fn normalize_doctor_category_accepts_completed_without_commit() {
        for alias in [
            "completed-without-commit",
            "completed-no-commit",
            "uncorroborated-completed",
            "integrity",
        ] {
            assert_eq!(
                normalize_doctor_category(alias).unwrap(),
                "completed-without-commit"
            );
        }
    }

    #[test]
    fn doctor_category_selected_honours_filter() {
        // No filter selects everything.
        assert!(doctor_category_selected(None, "completed-without-commit").unwrap());
        // Matching (including via alias) selects.
        assert!(doctor_category_selected(Some("integrity"), "completed-without-commit").unwrap());
        // Non-matching filter excludes.
        assert!(
            !doctor_category_selected(Some("stale-leases"), "completed-without-commit").unwrap()
        );
        // Unknown filter is an error, not a silent false.
        assert!(doctor_category_selected(Some("bogus"), "completed-without-commit").is_err());
    }

    #[test]
    fn parse_trace_id_token_extracts_spec_id() {
        assert_eq!(
            parse_trace_id_token("trace:TASK-673"),
            Some("TASK-673".into())
        );
        assert_eq!(
            parse_trace_id_token("trace:TASK-673"),
            Some("TASK-673".to_string()),
            "bare ids must keep the byte-identical normalized form"
        );
        assert_eq!(
            parse_trace_id_token("trace:TASK-673."),
            Some("TASK-673".to_string()),
            "sentence punctuation after a bare id must not become an empty criterion suffix"
        );
        assert_eq!(
            parse_trace_id_token("trace:TASK-673,"),
            Some("TASK-673".to_string())
        );
        assert_eq!(
            parse_trace_id_token("trace:TASK-673 | ai:claude"),
            Some("TASK-673".to_string())
        );
        // git grep on a ref may keep a `ref:file:` prefix — take the last trace:.
        assert_eq!(
            parse_trace_id_token("main:src/a.rs:trace:STORY-86"),
            Some("STORY-86".into())
        );
        // Hierarchical ids (FR-1-042) survive.
        assert_eq!(
            parse_trace_id_token("trace:FR-1-042"),
            Some("FR-1-042".into())
        );
        // Criterion-suffixed ids ride the same parser for test traces.
        assert_eq!(
            parse_trace_id_token("trace:STORY-1178.A1"),
            Some("STORY-1178.A1".into())
        );
        assert_eq!(
            parse_trace_id_token("trace:STORY-1178.ac1a2b3"),
            Some("STORY-1178.AC1A2B3".into())
        );
        // Lower-case input normalizes up.
        assert_eq!(parse_trace_id_token("trace:task-9"), Some("TASK-9".into()));
        // Malformed (no hyphen / leading digit) → None.
        assert_eq!(parse_trace_id_token("trace:nope"), None);
        assert_eq!(parse_trace_id_token("no trace here"), None);
    }

    /// Helper: a code repo on `main` plus an attached `.aida-store` whose specs
    /// are saved through `Storage`. Returns (project_root tempdir, storage).
    fn integrity_fixture() -> (tempfile::TempDir, Storage) {
        fn g(root: &std::path::Path, args: &[&str]) {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .expect("git");
            assert!(out.status.success(), "git {:?} failed", args);
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        g(root, &["init", "-q", "-b", "main"]);
        g(root, &["config", "user.email", "t@t.t"]);
        g(root, &["config", "user.name", "t"]);
        std::fs::create_dir_all(root.join(".aida-store")).unwrap();
        let storage = Storage::new(root.join(".aida-store"));
        (tmp, storage)
    }

    fn completed_spec(spec_id: &str) -> Requirement {
        let mut req = Requirement::new(format!("work for {spec_id}"), String::new());
        req.spec_id = Some(spec_id.to_string());
        req.status = RequirementStatus::Completed;
        req
    }

    #[test]
    fn integrity_flags_completed_spec_without_any_reference() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        let g = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
        };
        // A real commit corroborates TASK-100; TASK-200 has no commit at all.
        std::fs::write(root.join("a.txt"), "x").unwrap();
        g(&["add", "."]);
        g(&["commit", "-qm", "feat: do the thing (TASK-100)"]);

        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![completed_spec("TASK-100"), completed_spec("TASK-200")];
        storage.save(&store).unwrap();

        let findings = scan_completed_without_commit(root, &store, None);
        let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["TASK-200"],
            "only the uncorroborated spec flagged"
        );
        assert_eq!(findings[0].category, "completed-without-commit");
        assert!(!findings[0].safe_heal, "re-open is never a safe auto-fix");
    }

    #[test]
    fn integrity_trace_comment_corroborates() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        let g = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
        };
        // No (SPEC-ID) in the subject, but a tracked file carries the trace.
        std::fs::write(
            root.join("lib.rs"),
            "// trace:TASK-300 | ai:claude\nfn f() {}\n",
        )
        .unwrap();
        g(&["add", "."]);
        g(&["commit", "-qm", "chore: scaffold"]);

        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![completed_spec("TASK-300")];
        storage.save(&store).unwrap();

        let findings = scan_completed_without_commit(root, &store, None);
        assert!(
            findings.is_empty(),
            "a tracked // trace:SPEC-ID is corroboration, expected no findings, got {findings:?}"
        );
    }

    #[test]
    fn integrity_ignores_aida_store_bookkeeping_commits() {
        // The orphan-store-style bare "update TASK-NNN" subject must NOT count
        // as a reference — it always exists and would mask every violation.
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        let g = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
        };
        std::fs::write(root.join("a.txt"), "x").unwrap();
        g(&["add", "."]);
        // Bare "update TASK-400" — no parenthesized trailer.
        g(&["commit", "-qm", "update TASK-400"]);

        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![completed_spec("TASK-400")];
        storage.save(&store).unwrap();

        let findings = scan_completed_without_commit(root, &store, None);
        assert_eq!(
            findings.len(),
            1,
            "bare 'update TASK-NNN' must not corroborate"
        );
        assert_eq!(findings[0].id, "TASK-400");
    }

    #[test]
    fn integrity_done_specs_are_not_flagged() {
        // Done is the queue's "awaiting commit" transient, surfaced elsewhere —
        // this check targets only Completed-without-corroboration.
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "--allow-empty", "-qm", "init"])
            .output()
            .unwrap();

        let mut done = completed_spec("TASK-500");
        done.status = RequirementStatus::Done;
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![done];
        storage.save(&store).unwrap();

        assert!(scan_completed_without_commit(root, &store, None).is_empty());
    }

    // trace:TASK-1089 — the default cutoff is the fixed git-canonical migration
    // date, so pre-migration import-cohort completions are exempt by default.
    #[test]
    fn default_cutoff_is_the_fixed_migration_date() {
        assert_eq!(
            default_completed_without_commit_recent_cutoff().as_deref(),
            Some("2026-06-01T00:00:00Z"),
            "the default completed-without-commit cutoff must be the migration date"
        );
    }

    // trace:TASK-1089 — an ops/cleanup task opts out of the scan via the
    // explicit `doctor:no-code` tag; other tags / no tags do not.
    #[test]
    fn doctor_no_code_tag_opts_a_spec_out_of_the_scan() {
        use std::collections::HashSet;
        let with_optout: HashSet<String> = ["orchestrator", "doctor:no-code"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let without: HashSet<String> = ["orchestrator"].iter().map(|s| s.to_string()).collect();
        assert!(completed_without_commit_opted_out(&with_optout));
        assert!(!completed_without_commit_opted_out(&without));
        assert!(!completed_without_commit_opted_out(&HashSet::new()));
    }

    #[test]
    fn integrity_since_exempts_legacy_specs() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "--allow-empty", "-qm", "init"])
            .output()
            .unwrap();

        // TASK-1089: the legacy-exemption cutoff compares created_at, not
        // modified_at (the git-canonical migration bulk-reset modified_at to a
        // recent timestamp, defeating a modified_at cutoff). Set created_at.
        let mut legacy = completed_spec("TASK-600");
        legacy.created_at = "2020-01-01T00:00:00Z".parse().unwrap();
        let mut recent = completed_spec("TASK-601");
        recent.created_at = chrono::Utc::now();
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![legacy, recent];
        storage.save(&store).unwrap();

        // No cutoff → both flagged.
        assert_eq!(scan_completed_without_commit(root, &store, None).len(), 2);
        // Cutoff after the legacy spec → only the recent one flagged.
        let findings = scan_completed_without_commit(root, &store, Some("2023-01-01"));
        let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["TASK-601"]);
    }

    // trace:TASK-755 | ai:codex
    #[test]
    fn integrity_excludes_non_code_spec_types() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "--allow-empty", "-qm", "init"])
            .output()
            .unwrap();

        let mut task = completed_spec("TASK-700");
        task.req_type = RequirementType::Task;
        let mut decision = completed_spec("ADR-1");
        decision.req_type = RequirementType::Decision;
        let mut principle = completed_spec("PRIN-1");
        principle.req_type = RequirementType::Principle;
        let mut term = completed_spec("TERM-1");
        term.req_type = RequirementType::Term;
        let mut constraint = completed_spec("CON-1");
        constraint.req_type = RequirementType::Constraint;
        let mut vision = completed_spec("VIS-1");
        vision.req_type = RequirementType::Vision;
        let mut doc = completed_spec("DOC-1");
        doc.req_type = RequirementType::Doc;
        let mut meta = completed_spec("META-1");
        meta.req_type = RequirementType::Meta;
        let mut folder = completed_spec("FOLDER-1");
        folder.req_type = RequirementType::Folder;
        let mut epic = completed_spec("EPIC-1");
        epic.req_type = RequirementType::Epic;
        let mut spike = completed_spec("SPIKE-1");
        spike.req_type = RequirementType::Spike;

        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![
            task, decision, principle, term, constraint, vision, doc, meta, folder, epic, spike,
        ];
        storage.save(&store).unwrap();

        let findings = scan_completed_without_commit(root, &store, None);
        let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["TASK-700"]);
    }

    // trace:TASK-755 | ai:codex
    #[test]
    fn integrity_collapses_historical_tail_by_default() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "--allow-empty", "-qm", "init"])
            .output()
            .unwrap();

        // TASK-1089: the historical-tail cutoff compares created_at, not
        // modified_at (the migration bulk-reset modified_at to a recent value).
        let mut old = completed_spec("TASK-710");
        old.created_at = "2020-01-01T00:00:00Z".parse().unwrap();
        let mut recent = completed_spec("TASK-711");
        recent.created_at = chrono::Utc::now();
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![old, recent];
        storage.save(&store).unwrap();

        let scan =
            scan_completed_without_commit_with_options(root, &store, Some("2023-01-01"), false);
        let ids: Vec<&str> = scan.findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["TASK-711"]);
        assert_eq!(scan.hidden_older, 1);
    }

    // trace:TASK-755 | ai:codex
    #[test]
    fn integrity_all_expands_historical_tail() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "--allow-empty", "-qm", "init"])
            .output()
            .unwrap();

        let mut old = completed_spec("TASK-720");
        old.modified_at = "2020-01-01T00:00:00Z".parse().unwrap();
        let mut recent = completed_spec("TASK-721");
        recent.modified_at = chrono::Utc::now();
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![old, recent];
        storage.save(&store).unwrap();

        let scan =
            scan_completed_without_commit_with_options(root, &store, Some("2023-01-01"), true);
        let ids: Vec<&str> = scan.findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["TASK-720", "TASK-721"]);
        assert_eq!(scan.hidden_older, 0);
    }

    /// BUG-1637: the completed-without-commit heal never reopens the
    /// Completed spec. It reports, leaves the status, `completed_at` and the
    /// history untouched, and points at the recorded human reopen.
    // trace:TASK-673 trace:TASK-1477 trace:BUG-1637 | ai:claude
    #[test]
    fn integrity_heal_refuses_to_reopen_completed() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        let mut req = completed_spec("TASK-700");
        let stamp = chrono::Utc::now() - chrono::Duration::days(10);
        req.implementation_info = Some(aida_core::ImplementationInfo {
            completed_at: Some(stamp),
            completion_sha: Some("deadbeef".to_string()),
            ..Default::default()
        });
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![req];
        storage.save(&store).unwrap();

        let finding = DoctorFinding {
            category: "completed-without-commit".to_string(),
            id: "TASK-700".to_string(),
            summary: "Completed spec TASK-700 has no commit ...".to_string(),
            action: "re-open".to_string(),
            safe_heal: false,
        };
        let result = heal_doctor_completed_without_commit(root, &finding).unwrap();
        assert_eq!(result.status, "skipped");
        assert!(
            result.action.contains("never reopens a terminal status"),
            "{result:?}"
        );
        assert!(
            result
                .detail
                .as_deref()
                .unwrap_or("")
                .contains("aida edit TASK-700 --status done --force"),
            "the report names the recorded human reopen: {result:?}"
        );

        let reloaded = storage.load().unwrap();
        let r = &reloaded.requirements[0];
        assert_eq!(r.status, RequirementStatus::Completed);
        assert!(r.history.is_empty(), "nothing was written");
        assert_eq!(
            r.implementation_info.as_ref().unwrap().completed_at,
            Some(stamp)
        );
    }

    /// BUG-1637: the doctor's status-drift heal records its move under the
    /// doctor's automated author, so a terminal status the other clone reached
    /// meanwhile survives the merge (both orientations). It only moves
    /// Approved/In Progress, so it never leaves a terminal status itself.
    // trace:BUG-1637 | ai:claude
    #[test]
    fn doctor_status_heal_records_automated_author_and_keeps_terminal_on_merge() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        let mut base = completed_spec("TASK-1637");
        base.status = RequirementStatus::InProgress;
        base.modified_at = chrono::Utc::now() - chrono::Duration::hours(1);
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![base.clone()];
        storage.save(&store).unwrap();

        let finding = DoctorFinding {
            category: "spec-status-drift".to_string(),
            id: "TASK-1637".to_string(),
            summary: "In Progress with no active lease".to_string(),
            action: "revert to Approved (no active lease)".to_string(),
            safe_heal: true,
        };
        let result = heal_doctor_spec_status(root, &finding).unwrap();
        assert_eq!(result.status, "healed", "{result:?}");
        let healed = storage.load().unwrap().requirements[0].clone();
        assert_eq!(healed.status, RequirementStatus::Approved);
        let entry = healed.history.last().expect("a status history entry");
        assert_eq!(entry.author, aida_core::conflict::DOCTOR_AUTHOR);
        assert!(aida_core::conflict::is_automated_status_author(
            &entry.author
        ));

        let mut completed = base.clone();
        aida_core::conflict::set_status_recorded(
            &mut completed,
            RequirementStatus::Completed,
            "joe",
        );
        completed.modified_at = base.modified_at + chrono::Duration::minutes(1);
        for (ours, theirs) in [(&healed, &completed), (&completed, &healed)] {
            assert_eq!(
                aida_core::conflict::merge_spec_three_way(&base, ours, theirs).status,
                RequirementStatus::Completed
            );
        }

        // A terminal spec is never touched by this heal.
        let mut store = storage.load().unwrap();
        store.requirements[0].status = RequirementStatus::Rejected;
        storage.save(&store).unwrap();
        let again = heal_doctor_spec_status(root, &finding).unwrap();
        assert_eq!(again.status, "skipped");
        assert_eq!(
            storage.load().unwrap().requirements[0].status,
            RequirementStatus::Rejected
        );
    }

    /// BUG-407: `confirm_doctor_category` must NOT block on stdin in a
    /// non-interactive shell (the `aida doctor --heal` hang). Under `cargo
    /// test` stdin is non-interactive, so this returns Ok(false) immediately
    /// and the test completing at all proves it doesn't block. Also locks the
    /// contract: non-interactive declines (never silently auto-confirms).
    /// The open-socket hang condition itself is verified empirically (a
    // background `--heal` run with a finding present). trace:BUG-407 | ai:claude
    #[test]
    fn confirm_doctor_category_declines_in_non_interactive_shell() {
        assert!(!confirm_doctor_category("stale-leases", 3).unwrap());
    }

    // ------------------------------------------------------------------
    // STORY-666: destructive `--heal` requires sign-off in autonomous
    // contexts; safe fixes proceed everywhere. trace:STORY-666 | ai:claude
    // ------------------------------------------------------------------

    /// The pure disposition matrix — the single place the fail-closed invariant
    /// lives. Locks every cell.
    #[test]
    fn doctor_heal_disposition_matrix() {
        use HealDisposition::*;
        // Safe fixes ALWAYS proceed — every context, regardless of flags.
        for &force in &[false, true] {
            for &yes in &[false, true] {
                for &auto in &[false, true] {
                    assert_eq!(
                        doctor_heal_disposition(true, force, yes, auto),
                        Proceed,
                        "safe heal must proceed (force={force} yes={yes} auto={auto})"
                    );
                }
            }
        }
        // Destructive without the --force --yes opt-in → the pre-existing
        // needs-a-decision skip, in EVERY context (unchanged).
        for &auto in &[false, true] {
            assert_eq!(
                doctor_heal_disposition(false, false, false, auto),
                SkipNeedsForce
            );
            assert_eq!(
                doctor_heal_disposition(false, true, false, auto),
                SkipNeedsForce
            );
            assert_eq!(
                doctor_heal_disposition(false, false, true, auto),
                SkipNeedsForce
            );
        }
        // Destructive + --force --yes + INTERACTIVE (TTY) = explicit human
        // sign-off → proceeds (req #3: interactive path unchanged).
        assert_eq!(doctor_heal_disposition(false, true, true, false), Proceed);
        // Destructive + --force --yes + AUTONOMOUS = fail-closed gate (the
        // invariant): the heal must NOT execute.
        assert_eq!(
            doctor_heal_disposition(false, true, true, true),
            GateAutonomous
        );
    }

    /// The autonomous-context detector: no TTY OR a live orchestrator.
    #[test]
    fn doctor_context_is_autonomous_signals() {
        // Interactive TTY, no orchestrator → attended.
        assert!(!doctor_context_is_autonomous(true, false));
        // No TTY (piped / CI) → autonomous.
        assert!(doctor_context_is_autonomous(false, false));
        // Live orchestrator drain even at a "TTY" → autonomous.
        assert!(doctor_context_is_autonomous(true, true));
        assert!(doctor_context_is_autonomous(false, true));
    }

    /// End-to-end: in a simulated non-interactive context (under `cargo test`
    /// stdin is not a TTY, and the tempdir has no live drain → autonomous),
    /// a DESTRUCTIVE finding requested with `--force --yes` is GATED — the heal
    /// never executes and the spec is left untouched. This is the heart of the
    /// safety rail: it FAILS against the old code (which would re-open the spec)
    // and PASSES with the gate. trace:STORY-666 | ai:claude
    #[test]
    fn destructive_heal_is_gated_in_autonomous_context() {
        let (tmp, storage) = integrity_fixture();
        let root = tmp.path();
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![completed_spec("TASK-666")];
        storage.save(&store).unwrap();

        // `completed-without-commit` is a destructive (safe_heal=false) category
        // (its heal used to re-open a Completed spec; since BUG-1637 it only
        // reports, but the category stays gated).
        let finding = DoctorFinding {
            category: "completed-without-commit".to_string(),
            id: "TASK-666".to_string(),
            summary: "Completed spec TASK-666 has no commit".to_string(),
            action: "re-open".to_string(),
            safe_heal: false,
        };

        // --force --yes requested, but we are unattended (no TTY under test).
        let results = heal_doctor_findings(
            root,
            &[finding],
            &DoctorRunOptions {
                heal: true,
                yes: true,
                category: Some("completed-without-commit".to_string()),
                json: false,
                force: true,
                all: false,
                since: None,
                fail_on_findings: false,
            },
        )
        .unwrap();

        // The category was gated (skipped), not healed.
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, "skipped");
        assert!(
            results[0].action.contains("gated"),
            "expected a gate result, got: {:?}",
            results[0]
        );
        assert!(
            results[0]
                .detail
                .as_deref()
                .unwrap_or("")
                .contains("--force --yes --category"),
            "gate detail must name the interactive resume command"
        );

        // The destructive action did NOT run: the spec is still Completed.
        let reloaded = storage.load().unwrap();
        assert_eq!(
            reloaded.requirements[0].status,
            RequirementStatus::Completed,
            "destructive heal must NOT have executed in the autonomous context"
        );
    }

    /// End-to-end companion: in the SAME autonomous context, a SAFE
    /// (reversible) fix DOES proceed — the gate is scoped to the destructive
    /// subset only and must never over-gate routine reversible fixes.
    // trace:STORY-666 | ai:claude
    #[test]
    fn safe_heal_proceeds_in_autonomous_context() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        std::fs::create_dir_all(project_root.join(".aida")).unwrap();
        std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();
        let storage = Storage::new(project_root.join(".aida-store"));
        let mut req = Requirement::new("orphaned in-progress".into(), String::new());
        req.spec_id = Some("TASK-667".into());
        req.status = RequirementStatus::InProgress;
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![req];
        storage.save(&store).unwrap();

        // `spec-status-drift` (in-progress with no lease → revert to Approved)
        // is a SAFE (safe_heal=true) reversible fix.
        let finding = DoctorFinding {
            category: "spec-status-drift".to_string(),
            id: "TASK-667".to_string(),
            summary: "TASK-667 is In Progress but no active lease holds it".to_string(),
            action: "revert spec to Approved (no active lease found)".to_string(),
            safe_heal: true,
        };

        // Note: no --force, --yes set (so the safe-category TTY confirm is
        // skipped) — and we are non-interactive (autonomous) under test.
        let results = heal_doctor_findings(
            project_root,
            &[finding],
            &DoctorRunOptions {
                heal: true,
                yes: true,
                category: Some("spec-status-drift".to_string()),
                json: false,
                force: false,
                all: false,
                since: None,
                fail_on_findings: false,
            },
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].status, "healed",
            "safe reversible fix must proceed in the autonomous context, got: {:?}",
            results[0]
        );
        let reloaded = storage.load().unwrap();
        assert_eq!(reloaded.requirements[0].status, RequirementStatus::Approved);
    }

    // trace:STORY-1128 | ai:codex
    #[test]
    fn permission_posture_heal_applies_contained_writer() {
        let dir = tempfile::tempdir().unwrap();
        let finding = DoctorFinding {
            category: "permission-posture".to_string(),
            id: "codex-full-access-no-sandbox".to_string(),
            summary: "Codex full access without sandbox".to_string(),
            action: "apply contained posture".to_string(),
            safe_heal: true,
        };

        let result = heal_doctor_finding(
            dir.path(),
            &finding,
            &DoctorRunOptions {
                heal: true,
                yes: true,
                category: Some("permission-posture".to_string()),
                json: false,
                force: false,
                all: false,
                since: None,
                fail_on_findings: false,
            },
        )
        .unwrap();

        assert_eq!(result.status, "healed");
        let agents = std::fs::read_to_string(dir.path().join(".aida/agents.toml")).unwrap();
        let codex = std::fs::read_to_string(dir.path().join(".codex/config.toml")).unwrap();
        assert!(agents.contains("contained = true"), "{agents}");
        assert!(
            codex.contains("sandbox_mode = \"workspace-write\""),
            "{codex}"
        );
        assert!(dir.path().join(".aida/agents.toml.bak").exists());
        assert!(dir.path().join(".codex/config.toml.bak").exists());
    }

    #[test]
    fn doctor_detects_and_heals_stale_cache_lock_info() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        std::fs::create_dir_all(project_root.join(".aida")).unwrap();
        std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();
        let cache_path =
            aida_core::CachedGitBackend::default_cache_path(&project_root.join(".aida-store"));
        let lock_info_path = aida_core::cache_lock_info_path(&cache_path);
        let info = aida_core::CacheLockInfo {
            pid: 999_999,
            command: "aida list".to_string(),
            started_at: (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339(),
            user: "tester".to_string(),
            session_id: None,
            ..Default::default()
        };
        std::fs::write(&lock_info_path, serde_json::to_string(&info).unwrap()).unwrap();

        let findings = collect_doctor_findings(
            project_root,
            &aida_core::models::RequirementsStore::new(),
            Some("stale-locks"),
        )
        .unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, "stale-locks");
        assert!(findings[0].summary.contains("dead pid"));

        let result = heal_doctor_finding(
            project_root,
            &findings[0],
            &DoctorRunOptions {
                heal: true,
                yes: true,
                category: Some("stale-locks".to_string()),
                json: false,
                force: false,
                all: false,
                since: None,
                fail_on_findings: false,
            },
        )
        .unwrap();
        assert_eq!(result.status, "healed");
        assert!(!lock_info_path.exists());
    }

    // TASK-1484: a LIVE owner past its expected duration is reported as
    // diagnostic evidence but never healed; a reused PID reads as dead.
    // trace:TASK-1484 | ai:claude
    #[cfg(unix)]
    #[test]
    fn doctor_reports_live_lock_overrun_without_healing_it() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        std::fs::create_dir_all(project_root.join(".aida")).unwrap();
        std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();
        let cache_path =
            aida_core::CachedGitBackend::default_cache_path(&project_root.join(".aida-store"));
        let lock_info_path = aida_core::cache_lock_info_path(&cache_path);
        // pid 1 is always alive; a legacy (identity-less) record is PID-only.
        let info = aida_core::CacheLockInfo {
            pid: 1,
            command: "aida cache rebuild".to_string(),
            started_at: (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339(),
            user: "tester".to_string(),
            expected_duration_secs: Some(60),
            phase: Some("rebuild cache".to_string()),
            ..Default::default()
        };
        std::fs::write(&lock_info_path, serde_json::to_string(&info).unwrap()).unwrap();

        let findings = collect_doctor_findings(
            project_root,
            &aida_core::models::RequirementsStore::new(),
            Some("stale-locks"),
        )
        .unwrap();
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert!(!findings[0].safe_heal, "a live lock is never a safe heal");
        assert!(
            findings[0].summary.contains("live pid 1"),
            "{}",
            findings[0].summary
        );
        assert!(
            findings[0].summary.contains("diagnostic only"),
            "{}",
            findings[0].summary
        );

        let result = heal_doctor_stale_lock(&findings[0]).unwrap();
        assert_eq!(result.status, "skipped");
        assert!(
            lock_info_path.exists(),
            "live owner's lock-info must remain"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn doctor_treats_reused_pid_lock_info_as_dead() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        std::fs::create_dir_all(project_root.join(".aida")).unwrap();
        std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();
        let cache_path =
            aida_core::CachedGitBackend::default_cache_path(&project_root.join(".aida-store"));
        let lock_info_path = aida_core::cache_lock_info_path(&cache_path);
        let info = aida_core::CacheLockInfo {
            pid: 1,
            command: "aida schedule tick --hook".to_string(),
            started_at: (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339(),
            user: "tester".to_string(),
            pid_start_identity: Some("linux-starttime:18446744073709551615".to_string()),
            ..Default::default()
        };
        std::fs::write(&lock_info_path, serde_json::to_string(&info).unwrap()).unwrap();

        let findings = collect_doctor_findings(
            project_root,
            &aida_core::models::RequirementsStore::new(),
            Some("stale-locks"),
        )
        .unwrap();
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert!(findings[0].safe_heal);
        assert!(findings[0].summary.contains("different process"));
        let result = heal_doctor_stale_lock(&findings[0]).unwrap();
        assert_eq!(result.status, "healed");
        assert!(!lock_info_path.exists());
    }

    #[test]
    fn doctor_heals_in_progress_without_lease_back_to_approved() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        std::fs::create_dir_all(project_root.join(".aida")).unwrap();
        std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();
        let storage = Storage::new(project_root.join(".aida-store"));
        let mut req = Requirement::new("orphaned in-progress".into(), String::new());
        req.spec_id = Some("TASK-561".into());
        req.status = RequirementStatus::InProgress;
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![req];
        storage.save(&store).unwrap();

        let finding = DoctorFinding {
            category: "spec-status-drift".to_string(),
            id: "TASK-561".to_string(),
            summary: "TASK-561 is In Progress but no active lease holds it".to_string(),
            action: "revert spec to Approved (no active lease found)".to_string(),
            safe_heal: true,
        };
        let result = heal_doctor_finding(
            project_root,
            &finding,
            &DoctorRunOptions {
                heal: true,
                yes: true,
                category: Some("spec-status-drift".to_string()),
                json: false,
                force: false,
                all: false,
                since: None,
                fail_on_findings: false,
            },
        )
        .unwrap();

        assert_eq!(result.status, "healed");
        assert_eq!(
            result.action,
            "reverted TASK-561 from In Progress to Approved (no active lease found)"
        );
        let reloaded = storage.load().unwrap();
        assert_eq!(reloaded.requirements[0].status, RequirementStatus::Approved);
    }

    #[test]
    fn doctor_skips_stale_in_progress_orphan_finding_after_manual_fix() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        std::fs::create_dir_all(project_root.join(".aida")).unwrap();
        std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();
        let storage = Storage::new(project_root.join(".aida-store"));
        let mut req = Requirement::new("already fixed".into(), String::new());
        req.spec_id = Some("TASK-561".into());
        req.status = RequirementStatus::Approved;
        let mut store = aida_core::models::RequirementsStore::new();
        store.requirements = vec![req];
        storage.save(&store).unwrap();

        let finding = DoctorFinding {
            category: "spec-status-drift".to_string(),
            id: "TASK-561".to_string(),
            summary: "TASK-561 is In Progress but no active lease holds it".to_string(),
            action: "revert spec to Approved (no active lease found)".to_string(),
            safe_heal: true,
        };
        let result = heal_doctor_finding(
            project_root,
            &finding,
            &DoctorRunOptions {
                heal: true,
                yes: true,
                category: Some("spec-status-drift".to_string()),
                json: false,
                force: false,
                all: false,
                since: None,
                fail_on_findings: false,
            },
        )
        .unwrap();

        assert_eq!(result.status, "skipped");
        let reloaded = storage.load().unwrap();
        assert_eq!(reloaded.requirements[0].status, RequirementStatus::Approved);
    }

    // TASK-570: doctor detects orphan queue entries — rows whose backing
    // spec is no longer in the store. trace:TASK-570 | ai:claude
    #[test]
    fn doctor_detects_orphan_queue_entries() {
        // STORY-1429: `current_user_id` reads AIDA_USER, which sibling tests
        // set under the test env lock; hold it so the queue key cannot change
        // between this test's write and the doctor's read.
        // trace:STORY-1429 | ai:claude
        let _env = crate::test_env::env_lock();
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        std::fs::create_dir_all(project_root.join(".aida")).unwrap();
        std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();

        let storage = Storage::new(project_root.join(".aida-store"));
        let store = aida_core::models::RequirementsStore::new();
        storage.save(&store).unwrap();

        let user_id = current_user_id(None);
        let orphan_id = uuid::Uuid::new_v4();
        let entry = aida_core::models::QueueEntry {
            user_id: user_id.clone(),
            requirement_id: orphan_id,
            position: 1,
            added_by: user_id.clone(),
            note: Some("auto-queued by aida pr".into()),
            added_at: chrono::Utc::now(),
            for_role: Some("reviewer".into()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        };
        storage.queue_add(entry).unwrap();

        let findings =
            collect_doctor_findings(project_root, &store, Some("orphan-queue-entries")).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, "orphan-queue-entries");
        assert_eq!(findings[0].id, orphan_id.to_string());
        assert!(findings[0].summary.contains("position 1"));
        assert!(findings[0].summary.contains("[for:reviewer]"));
        assert!(findings[0].summary.contains("auto-queued by aida pr"));
        assert!(findings[0].safe_heal);
    }

    // TASK-570: --heal removes the orphan queue entry, mirroring the
    // behaviour of `aida queue prune --orphaned`. Re-running on a clean
    // queue is a no-op (skipped). trace:TASK-570 | ai:claude
    #[test]
    fn doctor_heals_orphan_queue_entries() {
        // Same AIDA_USER race as the detect test above. trace:STORY-1429 | ai:claude
        let _env = crate::test_env::env_lock();
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        std::fs::create_dir_all(project_root.join(".aida")).unwrap();
        std::fs::create_dir_all(project_root.join(".aida-store")).unwrap();

        let storage = Storage::new(project_root.join(".aida-store"));
        let store = aida_core::models::RequirementsStore::new();
        storage.save(&store).unwrap();

        let user_id = current_user_id(None);
        let orphan_id = uuid::Uuid::new_v4();
        let entry = aida_core::models::QueueEntry {
            user_id: user_id.clone(),
            requirement_id: orphan_id,
            position: 1,
            added_by: user_id.clone(),
            note: None,
            added_at: chrono::Utc::now(),
            for_role: Some("reviewer".into()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        };
        storage.queue_add(entry).unwrap();

        let finding = DoctorFinding {
            category: "orphan-queue-entries".to_string(),
            id: orphan_id.to_string(),
            summary: "queue position 1 [for:reviewer] points at deleted spec".to_string(),
            action: "remove orphan queue entry (aida queue prune --orphaned)".to_string(),
            safe_heal: true,
        };
        let opts = DoctorRunOptions {
            heal: true,
            yes: true,
            category: Some("orphan-queue-entries".to_string()),
            json: false,
            force: false,
            all: false,
            since: None,
            fail_on_findings: false,
        };
        let result = heal_doctor_finding(project_root, &finding, &opts).unwrap();
        assert_eq!(result.status, "healed");
        assert_eq!(result.action, "removed orphan queue entry");

        let entries = storage.queue_list(&user_id, false).unwrap();
        assert!(
            entries.iter().all(|e| e.requirement_id != orphan_id),
            "orphan entry should be removed from the queue"
        );

        // Idempotent: a second heal on an already-clean queue reports skipped.
        let result2 = heal_doctor_finding(project_root, &finding, &opts).unwrap();
        assert_eq!(result2.status, "skipped");
    }

    // TASK-570: the orphan-queue-entries alias set normalizes the way the
    // other doctor categories do. trace:TASK-570 | ai:claude
    #[test]
    fn doctor_orphan_queue_aliases_normalize() {
        assert_eq!(
            normalize_doctor_category("orphan-queue").unwrap(),
            "orphan-queue-entries"
        );
        assert_eq!(
            normalize_doctor_category("queue-orphans").unwrap(),
            "orphan-queue-entries"
        );
        assert_eq!(
            normalize_doctor_category("orphan_queue_entry").unwrap(),
            "orphan-queue-entries"
        );
    }

    #[test]
    fn salvage_component_sanitizes_path_like_values() {
        assert_eq!(sanitize_salvage_component("TASK-515/../x"), "TASK-515-x");
        assert_eq!(sanitize_salvage_component("advisor role"), "advisor-role");
    }

    #[test]
    fn salvage_worktree_patch_writes_diff_before_cleanup() {
        let project = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(worktree.path())
            .arg("init")
            .output()
            .unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(worktree.path())
            .args(["config", "user.email", "t@example.com"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(worktree.path())
            .args(["config", "user.name", "Test"])
            .output()
            .unwrap();
        let file = worktree.path().join("notes.txt");
        std::fs::write(&file, "before\n").unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(worktree.path())
            .args(["add", "notes.txt"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(worktree.path())
            .args(["commit", "-m", "init"])
            .output()
            .unwrap();
        std::fs::write(&file, "after\n").unwrap();

        let patch =
            salvage_worktree_patch(project.path(), "TASK-515", Some("codex"), worktree.path())
                .unwrap()
                .expect("dirty worktree should produce salvage patch");
        let body = std::fs::read_to_string(&patch).unwrap();
        assert!(patch
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("TASK-515-codex-attempt-"));
        assert!(body.contains("# AIDA salvage patch"));
        assert!(body.contains("-before"));
        assert!(body.contains("+after"));
    }

    // BUG-696: the salvage patch must capture untracked files' CONTENT, not
    // just their names — otherwise `aida doctor --heal` loses them when it
    // tears down the orphan-worktree after salvage.
    #[test]
    fn salvage_worktree_patch_captures_untracked_file_content() {
        let project = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        for args in [
            vec!["init"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "Test"],
        ] {
            std::process::Command::new("git")
                .arg("-C")
                .arg(worktree.path())
                .args(&args)
                .output()
                .unwrap();
        }
        // Commit a file, then dirty it (guarantees salvage runs) AND drop an
        // untracked file whose content must survive.
        let tracked = worktree.path().join("tracked.txt");
        std::fs::write(&tracked, "before\n").unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(worktree.path())
            .args(["add", "tracked.txt"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(worktree.path())
            .args(["commit", "-m", "init"])
            .output()
            .unwrap();
        std::fs::write(&tracked, "after\n").unwrap();
        std::fs::write(
            worktree.path().join("new_idea.txt"),
            "SALVAGE_ME_UNIQUE_MARKER untracked body\n",
        )
        .unwrap();

        let patch = salvage_worktree_patch(project.path(), "TASK-1", None, worktree.path())
            .unwrap()
            .expect("dirty worktree should produce salvage patch");
        let body = std::fs::read_to_string(&patch).unwrap();
        assert!(body.contains("new_idea.txt"), "untracked filename indexed");
        assert!(
            body.contains("SALVAGE_ME_UNIQUE_MARKER"),
            "untracked file CONTENT must be captured, not just the name:\n{body}"
        );
    }

    // ── TASK-752: legacy-store-cruft detection + guard ──

    #[test]
    fn normalize_doctor_category_accepts_legacy_store_cruft() {
        // TASK-752
        for alias in [
            "legacy-store-cruft",
            "legacy-store",
            "store-cruft",
            "legacy-cruft",
            "requirements-yaml",
        ] {
            assert_eq!(
                normalize_doctor_category(alias).unwrap(),
                "legacy-store-cruft"
            );
        }
    }

    #[test]
    fn is_legacy_store_cruft_path_matches_top_level_only() {
        // TASK-752: top-level legacy artifacts match.
        assert!(is_legacy_store_cruft_path("requirements.yaml"));
        assert!(is_legacy_store_cruft_path(
            "requirements_20251206_205840.yaml"
        ));
        assert!(is_legacy_store_cruft_path("scaffold-report.html"));
        // Nested paths and unrelated files never match.
        assert!(!is_legacy_store_cruft_path(
            "tests/fixtures/requirements.yaml"
        ));
        assert!(!is_legacy_store_cruft_path("src/requirements.yaml"));
        assert!(!is_legacy_store_cruft_path("my-requirements.yaml"));
        assert!(!is_legacy_store_cruft_path("requirements.json"));
        assert!(!is_legacy_store_cruft_path("docs/scaffold-report.html"));
    }

    fn git752(root: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {:?}: {:?}", args, out);
    }

    /// TASK-752: a distributed/git-canonical project (config declares
    /// `mode = "distributed"` AND the orphan `aida-store` branch exists) with a
    /// tracked `requirements.yaml` + `scaffold-report.html` flags both, and the
    /// heal git-rm's them + appends the gitignore guard.
    #[test]
    fn detects_and_heals_legacy_store_cruft_on_git_canonical_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git752(root, &["init", "-q", "-b", "main"]);
        git752(root, &["config", "user.email", "t@t.t"]);
        git752(root, &["config", "user.name", "t"]);
        // git-canonical: config declares distributed mode...
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[store]\nmode = \"distributed\"\nstore_path = \".aida-store\"\n",
        )
        .unwrap();
        // ...and the orphan aida-store branch exists.
        std::fs::write(root.join("README.md"), "x\n").unwrap();
        git752(root, &["add", "."]);
        git752(root, &["commit", "-qm", "init"]);
        git752(root, &["branch", "aida-store"]);
        // Tracked legacy-store artifacts.
        std::fs::write(root.join("requirements.yaml"), "legacy: store\n").unwrap();
        std::fs::write(root.join("scaffold-report.html"), "<html/>\n").unwrap();
        git752(root, &["add", "requirements.yaml", "scaffold-report.html"]);
        git752(root, &["commit", "-qm", "cruft"]);

        // Detect.
        let store = aida_core::models::RequirementsStore::new();
        let findings = collect_doctor_findings(root, &store, Some("legacy-store-cruft")).unwrap();
        assert_eq!(findings.len(), 2, "both cruft files flagged: {findings:?}");
        assert!(findings.iter().all(|f| f.category == "legacy-store-cruft"));
        assert!(findings.iter().all(|f| f.safe_heal));
        let ids: std::collections::HashSet<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        assert!(ids.contains("requirements.yaml"));
        assert!(ids.contains("scaffold-report.html"));

        // Heal both.
        for finding in &findings {
            let result = heal_doctor_legacy_store_cruft(root, finding).unwrap();
            assert_eq!(result.status, "healed", "{result:?}");
        }
        // git-rm'd → no longer tracked → no longer flagged.
        let after = collect_doctor_findings(root, &store, Some("legacy-store-cruft")).unwrap();
        assert!(
            after.is_empty(),
            "healed cruft must not reappear: {after:?}"
        );
        // gitignore guard appended exactly PR-651's block.
        let gi = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(gi.contains("requirements*.yaml"));
        assert!(gi.contains("scaffold-report.html"));
    }

    /// TASK-752 GUARD: a legacy `--centralized` project that legitimately USES
    /// `requirements.yaml` as its active store (config does NOT declare
    /// distributed mode, no orphan aida-store branch) is a NO-OP — we must never
    /// flag/nuke an active centralized backend.
    #[test]
    fn legacy_store_cruft_is_noop_on_centralized_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git752(root, &["init", "-q", "-b", "main"]);
        git752(root, &["config", "user.email", "t@t.t"]);
        git752(root, &["config", "user.name", "t"]);
        // Legacy centralized: requirements.yaml is the ACTIVE store, no
        // distributed-mode config, no orphan aida-store branch.
        std::fs::write(
            root.join("requirements.yaml"),
            "active: centralized store\n",
        )
        .unwrap();
        git752(root, &["add", "."]);
        git752(root, &["commit", "-qm", "centralized"]);

        // No distributed-mode declaration → no orphan branch → no-op.
        assert!(
            detect_legacy_store_cruft(root).is_empty(),
            "must not flag an active centralized requirements.yaml"
        );
        let store = aida_core::models::RequirementsStore::new();
        let findings = collect_doctor_findings(root, &store, Some("legacy-store-cruft")).unwrap();
        assert!(findings.is_empty(), "centralized project: no findings");
    }

    /// TASK-752: even with distributed-mode config, if the orphan aida-store
    /// branch is MISSING (half-migrated config), stay silent — Gate 2.
    #[test]
    fn legacy_store_cruft_is_noop_without_orphan_branch() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git752(root, &["init", "-q", "-b", "main"]);
        git752(root, &["config", "user.email", "t@t.t"]);
        git752(root, &["config", "user.name", "t"]);
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[store]\nmode = \"distributed\"\n",
        )
        .unwrap();
        std::fs::write(root.join("requirements.yaml"), "x\n").unwrap();
        git752(root, &["add", "."]);
        git752(root, &["commit", "-qm", "init"]);
        // No `aida-store` branch created.
        assert!(
            detect_legacy_store_cruft(root).is_empty(),
            "missing orphan branch → no-op"
        );
    }

    // ── BUG-563: store-tracked per-clone runtime detection + heal ──

    #[test]
    fn normalize_doctor_category_accepts_store_tracked_runtime() {
        // BUG-563
        for alias in [
            "store-tracked-runtime",
            "store-runtime",
            "tracked-runtime",
            "store-node-toml",
            "store-runtime-cruft",
        ] {
            assert_eq!(
                normalize_doctor_category(alias).unwrap(),
                "store-tracked-runtime"
            );
        }
    }

    #[test]
    fn is_store_tracked_runtime_path_matches_per_clone_runtime() {
        // BUG-563: the per-clone runtime set under top-level `.aida/`.
        assert!(is_store_tracked_runtime_path(".aida/node.toml"));
        assert!(is_store_tracked_runtime_path(".aida/dispenser.toml"));
        assert!(is_store_tracked_runtime_path(".aida/dispenser.lock"));
        assert!(is_store_tracked_runtime_path(".aida/sync.lock"));
        assert!(is_store_tracked_runtime_path(".aida/cache.db"));
        assert!(is_store_tracked_runtime_path(".aida/cache.db-journal"));
        assert!(is_store_tracked_runtime_path(".aida/cache.db-wal"));
        assert!(is_store_tracked_runtime_path(".aida/cache.db-shm"));
        // Shared, legitimately-tracked store files never match.
        assert!(!is_store_tracked_runtime_path(".aida/config.toml"));
        assert!(!is_store_tracked_runtime_path("metadata.yaml"));
        assert!(!is_store_tracked_runtime_path(
            "objects/TASK/000/TASK-1.yaml"
        ));
        assert!(!is_store_tracked_runtime_path(
            "registry/agreed_counters.toml"
        ));
        // Not under .aida/, or nested deeper than top-level — never match.
        assert!(!is_store_tracked_runtime_path("node.toml"));
        assert!(!is_store_tracked_runtime_path(".aida/sub/node.toml"));
        assert!(!is_store_tracked_runtime_path("src/.aida/node.toml"));
    }

    /// BUG-563: build a project that declares distributed mode with an attached
    /// `.aida-store` worktree (orphan `aida-store` branch) whose tree TRACKS
    /// `.aida/node.toml` + `.aida/cache.db` + a `.aida/*.lock`. The detector
    /// flags all three; the heal `git rm --cached`'s + gitignores + commits on
    /// the store worktree, after which they no longer reappear and the working
    /// copies survive (per-clone state stays on disk).
    #[test]
    fn detects_and_heals_store_tracked_runtime_on_git_canonical_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git752(root, &["init", "-q", "-b", "main"]);
        git752(root, &["config", "user.email", "t@t.t"]);
        git752(root, &["config", "user.name", "t"]);
        // git-canonical: config declares distributed mode.
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[store]\nmode = \"distributed\"\nstore_path = \".aida-store\"\n",
        )
        .unwrap();
        std::fs::write(root.join("README.md"), "x\n").unwrap();
        git752(root, &["add", "."]);
        git752(root, &["commit", "-qm", "init"]);

        // Attach the orphan `aida-store` worktree at `.aida-store/`.
        git752(
            root,
            &["worktree", "add", "-q", "-b", "aida-store", ".aida-store"],
        );
        let store = root.join(".aida-store");
        // Make it look like a real store (the detector gates on objects/).
        std::fs::create_dir_all(store.join("objects")).unwrap();
        std::fs::write(store.join("objects/.keep"), "").unwrap();
        std::fs::create_dir_all(store.join(".aida")).unwrap();
        // Per-clone runtime files WRONGLY tracked on the orphan branch.
        std::fs::write(store.join(".aida/node.toml"), "node_id = 1\n").unwrap();
        std::fs::write(store.join(".aida/dispenser.toml"), "next = 5\n").unwrap();
        std::fs::write(store.join(".aida/sync.lock"), "").unwrap();
        std::fs::write(store.join(".aida/cache.db"), "binary").unwrap();
        git752(&store, &["add", "-A"]);
        git752(&store, &["commit", "-qm", "store w/ tracked runtime"]);

        // Detect: all four per-clone runtime files flagged.
        let req_store = aida_core::models::RequirementsStore::new();
        let findings =
            collect_doctor_findings(root, &req_store, Some("store-tracked-runtime")).unwrap();
        assert_eq!(findings.len(), 4, "all runtime files flagged: {findings:?}");
        assert!(findings
            .iter()
            .all(|f| f.category == "store-tracked-runtime"));
        assert!(findings.iter().all(|f| f.safe_heal));
        let ids: std::collections::HashSet<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        assert!(ids.contains(".aida/node.toml"));
        assert!(ids.contains(".aida/dispenser.toml"));
        assert!(ids.contains(".aida/sync.lock"));
        assert!(ids.contains(".aida/cache.db"));

        // Heal each.
        for finding in &findings {
            let result = heal_doctor_store_tracked_runtime(root, finding).unwrap();
            assert_eq!(result.status, "healed", "{result:?}");
        }
        // git-rm --cached'd → no longer tracked → no longer flagged.
        let after =
            collect_doctor_findings(root, &req_store, Some("store-tracked-runtime")).unwrap();
        assert!(
            after.is_empty(),
            "healed runtime must not reappear: {after:?}"
        );
        // Working copies survive (per-clone state stays on disk).
        assert!(store.join(".aida/node.toml").exists());
        assert!(store.join(".aida/cache.db").exists());
        // gitignore guard appended on the STORE worktree.
        let gi = std::fs::read_to_string(store.join(".gitignore")).unwrap();
        assert!(gi.contains(".aida/node.toml"));
        assert!(gi.contains(".aida/dispenser.toml"));
        assert!(gi.contains(".aida/*.lock"));
        assert!(gi.contains(".aida/cache.db"));
    }

    /// BUG-563 GUARD: with distributed-mode config but NO attached `.aida-store`
    /// worktree, the detector is a NO-OP (Gate 2) — nothing to scan.
    #[test]
    fn store_tracked_runtime_is_noop_without_store_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git752(root, &["init", "-q", "-b", "main"]);
        git752(root, &["config", "user.email", "t@t.t"]);
        git752(root, &["config", "user.name", "t"]);
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[store]\nmode = \"distributed\"\n",
        )
        .unwrap();
        std::fs::write(root.join("README.md"), "x\n").unwrap();
        git752(root, &["add", "."]);
        git752(root, &["commit", "-qm", "init"]);
        // No `.aida-store/` worktree attached.
        assert!(
            detect_store_tracked_runtime(root).is_empty(),
            "no store worktree → no-op"
        );
    }
}

/// STORY-70: walk the orphan store, flag STORY/BUG requirements whose
/// descriptions don't contain a recognized acceptance heading. Output
/// shape mirrors the other doctor commands (per-finding rows + a final
/// summary). Exits non-zero on findings so CI/scripts can gate on it.
// trace:STORY-70 | ai:claude
fn doctor_convention_check(quiet: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let objects_root = project_root.join(".aida-store").join("objects");
    if !objects_root.exists() {
        println!("(no objects/ tree — nothing to check)");
        return Ok(());
    }

    let reqs = aida_core::object_store::load_all_objects(&objects_root)?;
    let mut total_in_scope: usize = 0;
    let mut missing: Vec<(String, String)> = Vec::new();
    for req in &reqs {
        if !matches!(
            req.req_type,
            aida_core::RequirementType::Story | aida_core::RequirementType::Bug
        ) {
            continue;
        }
        total_in_scope += 1;
        if requirement_missing_acceptance(req) {
            let id = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());
            missing.push((id, req.title.clone()));
        }
    }
    missing.sort_by(|a, b| a.0.cmp(&b.0));

    if missing.is_empty() {
        println!(
            "{} all {} STORY/BUG description(s) carry an acceptance section.",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            total_in_scope
        );
        return Ok(());
    }

    if !quiet {
        for (id, title) in &missing {
            println!(
                "{} {}  no `## Acceptance` / `## Verify` section  {}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                id.bold(),
                title.dimmed()
            );
        }
    }
    println!(
        "{} of {} STORY/BUG descriptions missing acceptance criteria",
        format!("{}", missing.len()).bold(),
        total_in_scope
    );
    println!(
        "  ({})",
        "run `aida edit <id>` to add — STORY-67 will pick it up automatically".dimmed()
    );
    std::process::exit(1);
}

/// Walk every YAML in objects/, collect every `relationships[*].target_id`
/// reference, and verify each resolves to an existing req's UUID. Reports
/// dangling references, optionally repairs by stripping the bad entries.
///
/// TASK-58: rewrote the original line-scanner — which exited the
/// `relationships:` block on the first `- rel_type:` array entry
/// (any line starting with `-` matched the "exiting top-level key"
/// heuristic) and so never inspected any `target_id`. Now uses
/// `object_store::load_all_objects()` for proper serde-driven
// deserialization. trace:TASK-58 | ai:claude
fn doctor_verify_relationships(repair: bool, yes: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let store_path = project_root.join(".aida-store");
    let objects_root = store_path.join("objects");
    if !objects_root.exists() {
        println!("(no objects/ tree — nothing to check)");
        return Ok(());
    }

    let reqs = aida_core::object_store::load_all_objects(&objects_root)?;

    // First pass: collect every uuid present in the store + the spec
    // mapping for nicer dangling-edge reporting.
    let all_uuids: std::collections::HashSet<uuid::Uuid> = reqs.iter().map(|r| r.id).collect();
    let _spec_by_uuid: std::collections::HashMap<uuid::Uuid, String> = reqs
        .iter()
        .filter_map(|r| r.spec_id.as_ref().map(|s| (r.id, s.clone())))
        .collect();

    // Second pass: walk each req's relationships array and check each
    // target_id resolves to an existing uuid.
    #[derive(Debug)]
    struct Dangling {
        source_uuid: uuid::Uuid,
        source_spec: String,
        target_uuid: uuid::Uuid,
        rel_type: String,
    }
    let mut dangling: Vec<Dangling> = Vec::new();
    for req in &reqs {
        let source_spec = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());
        for rel in &req.relationships {
            if !all_uuids.contains(&rel.target_id) {
                dangling.push(Dangling {
                    source_uuid: req.id,
                    source_spec: source_spec.clone(),
                    target_uuid: rel.target_id,
                    rel_type: format!("{:?}", rel.rel_type),
                });
            }
        }
    }

    if dangling.is_empty() {
        println!(
            "{} every relationship target resolves to an existing requirement.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        return Ok(());
    }

    println!(
        "{} {} dangling relationship reference(s):",
        crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
        dangling.len()
    );
    println!();
    for d in &dangling {
        println!(
            "  {} → {}: target uuid {} not found",
            d.source_spec.bold(),
            d.rel_type.dimmed(),
            d.target_uuid.to_string().yellow()
        );
    }
    println!();

    if !repair {
        println!(
            "Run with {} to strip dangling references in-place.",
            "--repair".cyan()
        );
        std::process::exit(1);
    }

    if !yes {
        use std::io::Write;
        print!("Strip {} dangling reference(s)? [y/N] ", dangling.len());
        std::io::stdout().flush()?;
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Repair: filter dangling edges out of each affected req's
    // relationships array and rewrite the YAML. trace:TASK-58 | ai:claude
    let dangling_target_uuids: std::collections::HashSet<uuid::Uuid> =
        dangling.iter().map(|d| d.target_uuid).collect();
    let affected_source_uuids: std::collections::HashSet<uuid::Uuid> =
        dangling.iter().map(|d| d.source_uuid).collect();
    let mut fixed = 0usize;
    for mut req in reqs.into_iter() {
        if !affected_source_uuids.contains(&req.id) {
            continue;
        }
        let before = req.relationships.len();
        req.relationships
            .retain(|r| !dangling_target_uuids.contains(&r.target_id));
        if req.relationships.len() != before {
            aida_core::object_store::write_object(&store_path.join("objects"), &req)?;
            fixed += 1;
        }
    }
    let _ = aida_core::git_ops::add(&store_path, &["objects"]);
    let _ = aida_core::git_ops::commit(
        &store_path,
        &format!(
            "chore(repair): strip {} dangling relationship target(s)",
            dangling.len()
        ),
    );
    println!(
        "{} repaired {} requirement(s).",
        crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
        fixed
    );
    println!("  Push with: {}", "aida push".cyan());
    Ok(())
}

// Walk source files under the project root for `trace:<SPEC-ID>`
/// patterns and verify each spec_id resolves to a requirement in the
/// store. With `strip_dangling`, rewrites source files to remove the
// dangling trace markers. trace:EPIC-19 | ai:claude
fn doctor_validate_trace_comments(strip_dangling: bool, dry_run: bool, yes: bool) -> Result<()> {
    let project_root = find_project_root()?;
    let store_path = project_root.join(".aida-store");
    let objects_root = store_path.join("objects");
    if !objects_root.exists() {
        println!("(no objects/ tree — nothing to check)");
        return Ok(());
    }

    // Collect every spec_id AND agreed_id from the store. A trace
    // comment is considered valid if it matches either form — the
    // spec_id is the original (pre-merge-gate) id and stays in trace
    // comments, while agreed_id is the canonical post-merge form.
    // trace:EPIC-19 | ai:claude
    let mut yaml_files: Vec<std::path::PathBuf> = Vec::new();
    walk_yamls(&objects_root, &mut yaml_files);
    let mut known_specs: std::collections::HashSet<String> = std::collections::HashSet::new();
    for path in &yaml_files {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in content.lines() {
            let t = line.trim_start();
            if let Some(v) = t.strip_prefix("spec_id:") {
                let s = v.trim().trim_matches('"').trim_matches('\'').to_string();
                if !s.is_empty() {
                    known_specs.insert(s);
                }
            } else if let Some(v) = t.strip_prefix("agreed_id:") {
                let s = v.trim().trim_matches('"').trim_matches('\'').to_string();
                // agreed_id can be `null` / empty in YAML; skip those.
                if !s.is_empty() && s != "null" && s != "~" {
                    known_specs.insert(s);
                }
            }
        }
    }

    // Collect every trace comment in the project tree.
    let trace_re = regex::Regex::new(r"trace:([A-Z]+(?:-[A-Z0-9]+)?-[0-9]+(?:-[0-9]+)?)").unwrap();
    let mut by_spec: std::collections::HashMap<String, Vec<(std::path::PathBuf, usize)>> =
        std::collections::HashMap::new();

    walk_source_for_traces(&project_root, &trace_re, &mut by_spec);

    let mut orphan_specs: Vec<&String> = by_spec
        .keys()
        .filter(|s| !known_specs.contains(*s))
        .collect();
    orphan_specs.sort();

    if orphan_specs.is_empty() {
        println!(
            "{} every `trace:<SPEC-ID>` in source resolves to a requirement ({} unique spec_ids referenced from {} location(s)).",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            by_spec.len(),
            by_spec.values().map(|v| v.len()).sum::<usize>()
        );
        return Ok(());
    }

    println!(
        "{} {} trace comment(s) reference unknown spec_ids:",
        crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
        orphan_specs.len()
    );
    println!();
    for spec in &orphan_specs {
        let locations = by_spec.get(*spec).unwrap();
        println!("{}: ({} reference(s))", spec.bold(), locations.len());
        for (path, line) in locations.iter().take(5) {
            let rel = path.strip_prefix(&project_root).unwrap_or(path);
            println!("  {}:{}", rel.display(), line);
        }
        if locations.len() > 5 {
            println!("  … and {} more", locations.len() - 5);
        }
        println!();
    }
    if !strip_dangling {
        println!("Likely causes: req was deleted, or a typo. Either delete the");
        println!("trace comment or update it to reference an existing spec_id.");
        println!();
        println!(
            "To strip these in-place: {}",
            "aida doctor validate-trace-comments --strip-dangling".cyan()
        );
        std::process::exit(1);
    }

    // --- strip-dangling path ---
    let dangling_set: std::collections::HashSet<String> =
        orphan_specs.iter().map(|s| (*s).clone()).collect();

    println!(
        "{} {} reference(s) across {} unique spec_ids will be stripped.",
        "Plan:".yellow().bold(),
        by_spec
            .iter()
            .filter(|(s, _)| dangling_set.contains(*s))
            .map(|(_, v)| v.len())
            .sum::<usize>(),
        dangling_set.len()
    );

    if dry_run {
        println!();
        let stats = strip_dangling_traces(&project_root, &dangling_set, true)?;
        println!(
            "→ dry-run: would delete {} whole line(s) and modify {} other line(s) across {} file(s).",
            stats.lines_deleted, stats.lines_modified, stats.files_changed
        );
        return Ok(());
    }

    if !yes {
        use std::io::Write;
        print!("Strip dangling trace annotations from source files? [y/N] ");
        std::io::stdout().flush()?;
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let stats = strip_dangling_traces(&project_root, &dangling_set, false)?;
    println!(
        "{} stripped {} line(s) (deleted {} whole, modified {} mixed) across {} file(s).",
        crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
        stats.lines_deleted + stats.lines_modified,
        stats.lines_deleted,
        stats.lines_modified,
        stats.files_changed
    );
    println!("  Review the diff: {}", "git diff".cyan());
    Ok(())
}

#[derive(Default)]
struct StripStats {
    files_changed: usize,
    lines_deleted: usize,
    lines_modified: usize,
}

/// Walk every text source file under `root` and either delete or modify
// any line containing `trace:<DANGLING>` per the dangling_ids set.
// When `dry_run`, returns counts without writing. trace:EPIC-19
fn strip_dangling_traces(
    root: &std::path::Path,
    dangling_ids: &std::collections::HashSet<String>,
    dry_run: bool,
) -> Result<StripStats> {
    let mut stats = StripStats::default();
    strip_dangling_walk(root, dangling_ids, dry_run, &mut stats);
    Ok(stats)
}

fn strip_dangling_walk(
    root: &std::path::Path,
    dangling_ids: &std::collections::HashSet<String>,
    dry_run: bool,
    stats: &mut StripStats,
) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if path.is_dir() {
            if matches!(
                name,
                ".git"
                    | ".aida-store"
                    | ".aida"
                    | "target"
                    | "node_modules"
                    | "dist"
                    | "build"
                    | ".cache"
                    | ".venv"
                    | "venv"
            ) {
                continue;
            }
            strip_dangling_walk(&path, dangling_ids, dry_run, stats);
            continue;
        }
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let probably_text = matches!(
            ext,
            "rs" | "py"
                | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "go"
                | "java"
                | "c"
                | "cpp"
                | "h"
                | "hpp"
                | "cs"
                | "rb"
                | "sh"
                | "md"
                | "toml"
                | "yaml"
                | "yml"
                | "json"
        );
        if !probably_text {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Quick scan: any dangling id present?
        let mut had_match = false;
        for id in dangling_ids {
            if content.contains(&format!("trace:{}", id)) {
                had_match = true;
                break;
            }
        }
        if !had_match {
            continue;
        }

        let (new_content, deleted, modified) = rewrite_strip_dangling(&content, dangling_ids);
        if new_content == content {
            continue;
        }
        stats.files_changed += 1;
        stats.lines_deleted += deleted;
        stats.lines_modified += modified;
        if !dry_run {
            let _ = std::fs::write(&path, new_content);
        }
    }
}

/// Pure transform: take a file's content and the dangling-id set,
/// return (new_content, lines_deleted, lines_modified). A line is
/// "deleted" when the only meaningful content was the trace marker
/// (post-strip, only a comment marker remains); otherwise the trace
// fragment is excised and the line is "modified". trace:EPIC-19
fn rewrite_strip_dangling(
    content: &str,
    dangling_ids: &std::collections::HashSet<String>,
) -> (String, usize, usize) {
    use regex::Regex;
    // Match `trace:<ID> | ai:<tool>(:<conf>)?` fragments. Capture the
    // id so we can check it against the dangling set.
    let frag_re =
        Regex::new(r"trace:([A-Z]+(?:-[A-Z0-9]+)?-[0-9]+(?:-[0-9]+)?)\s*\|\s*ai:[a-zA-Z]+(?::(?:high|med|low))?")
            .unwrap();

    let mut out = String::with_capacity(content.len());
    let mut deleted = 0;
    let mut modified = 0;

    for line in content.lines() {
        // Does this line contain a dangling trace?
        let mut should_strip = false;
        for cap in frag_re.captures_iter(line) {
            if let Some(m) = cap.get(1) {
                if dangling_ids.contains(m.as_str()) {
                    should_strip = true;
                    break;
                }
            }
        }
        if !should_strip {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Strip every dangling trace fragment from this line.
        let stripped = frag_re
            .replace_all(line, |caps: &regex::Captures| {
                let id = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                if dangling_ids.contains(id) {
                    String::new()
                } else {
                    caps.get(0)
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_default()
                }
            })
            .into_owned();

        // Decide: delete the whole line if what remains is just a
        // comment marker (no content), or modify (keep the line with
        // the fragment removed).
        let trimmed = stripped.trim();
        let is_just_marker = matches!(trimmed, "" | "//" | "///" | "//!" | "/*" | "*/" | "*" | "#")
            || trimmed.starts_with("// ") && trimmed.trim_end_matches(' ').len() <= 3;

        if is_just_marker {
            deleted += 1;
            // skip — don't push this line
        } else {
            modified += 1;
            // Clean up double-spaces left behind by the strip.
            let cleaned = stripped.replace("  ", " ").trim_end().to_string();
            out.push_str(&cleaned);
            out.push('\n');
        }
    }

    // Preserve trailing newline behavior (str.lines() drops it; if the
    // original content ended without a newline, drop our trailing one).
    if !content.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }

    (out, deleted, modified)
}

/// Recursively collect every `*.yaml` file under `root` into `out`.
/// Hand-rolled to avoid adding a walkdir dep just for the doctor ops.
/// The orphan store's objects/ tree is shallow (3 levels) so a simple
// recursive read_dir is fine. trace:EPIC-19 | ai:claude
fn walk_yamls(root: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_yamls(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("yaml") {
            out.push(path);
        }
    }
}

/// Mark blocks whose owner isn't in nodes.toml as exhausted, so the
/// dispenser skips them but their range stays reserved (so other
/// clones don't reallocate the same numbers and create a real
// collision). trace:EPIC-19 | ai:claude
fn doctor_repair_stale_blocks(dry_run: bool, yes: bool) -> Result<()> {
    use aida_core::{BlockRegistry, NodeRegistry};

    let project_root = find_project_root()?;
    let store_path = project_root.join(".aida-store");
    let blocks_path = store_path.join("registry").join("blocks.yaml");
    let nodes_path = store_path.join("registry").join("nodes.toml");

    if !blocks_path.exists() {
        println!("(no blocks.yaml — nothing to repair)");
        return Ok(());
    }

    let mut blocks = BlockRegistry::load(&blocks_path).unwrap_or_default();
    let nodes = NodeRegistry::load(&nodes_path).unwrap_or_default();
    let registered: std::collections::HashSet<String> =
        nodes.nodes.iter().map(|n| n.id.clone()).collect();

    let stale: Vec<(usize, &aida_core::AgreedIdBlock)> = blocks
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| !registered.contains(&b.node_id) && !b.is_exhausted())
        .collect();

    if stale.is_empty() {
        println!(
            "{} no stale blocks — every active block has a registered node.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        return Ok(());
    }

    println!("{}", "Stale blocks (owner not in nodes.toml)".bold());
    for (_, b) in &stale {
        println!(
            "  {} node `{}` owns {}-{}..{} (next={})",
            "·".dimmed(),
            b.node_id,
            b.type_prefix,
            b.range_start,
            b.range_end,
            b.next
        );
    }
    println!();
    println!("Plan: bump each block's `next` past `range_end` so the dispenser");
    println!("      skips it. The range stays reserved (preserves cross-clone safety).");
    println!();

    if dry_run {
        println!("{} dry-run — no changes written.", "→".cyan());
        return Ok(());
    }
    if !yes {
        use std::io::Write;
        print!("Tombstone {} stale block(s)? [y/N] ", stale.len());
        std::io::stdout().flush()?;
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let stale_indices: Vec<usize> = stale.iter().map(|(i, _)| *i).collect();
    let count = stale_indices.len();
    for idx in stale_indices {
        let b = &mut blocks.blocks[idx];
        b.next = b.range_end + 1;
    }
    blocks.save(&blocks_path)?;
    let _ = aida_core::git_ops::add(&store_path, &["registry/blocks.yaml"]);
    let _ = aida_core::git_ops::commit(
        &store_path,
        &format!(
            "chore(registry): tombstone {} stale block(s) (no node owner)",
            count
        ),
    );

    println!(
        "{} tombstoned {} block(s).",
        crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
        count
    );
    println!("  Push with: {}", "aida push".cyan());
    Ok(())
}

/// Walk the orphan store's objects tree, group requirements by their
/// `spec_id` field, and report any spec_id claimed by more than one
/// requirement. v1 reports only — auto-renumber is dangerous (would
// orphan trace comments + commit refs). trace:EPIC-19 | ai:claude
// why: the one-off `collisions` borrow-tuple is local to this reporter; a named alias used in a single spot would obscure more than the inline type.
#[allow(clippy::type_complexity)]
fn doctor_scrub_collisions() -> Result<()> {
    let project_root = find_project_root()?;
    let store_path = project_root.join(".aida-store");
    let objects_root = store_path.join("objects");
    if !objects_root.exists() {
        println!("(no objects/ tree — nothing to check)");
        return Ok(());
    }

    let mut by_spec: std::collections::HashMap<String, Vec<(String, String, String)>> =
        std::collections::HashMap::new();

    let mut yaml_files: Vec<std::path::PathBuf> = Vec::new();
    walk_yamls(&objects_root, &mut yaml_files);
    for path in &yaml_files {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let mut spec_id = String::new();
        let mut uuid = String::new();
        let mut title = String::new();
        for raw in content.lines() {
            let line = raw.trim_start();
            if let Some(v) = line.strip_prefix("spec_id:") {
                spec_id = v.trim().trim_matches('"').trim_matches('\'').to_string();
            } else if let Some(v) = line.strip_prefix("id:") {
                if uuid.is_empty() {
                    uuid = v.trim().trim_matches('"').trim_matches('\'').to_string();
                }
            } else if let Some(v) = line.strip_prefix("title:") {
                if title.is_empty() {
                    title = v.trim().trim_matches('"').trim_matches('\'').to_string();
                }
            }
            if !spec_id.is_empty() && !uuid.is_empty() && !title.is_empty() {
                break;
            }
        }
        if spec_id.is_empty() {
            continue;
        }
        by_spec
            .entry(spec_id)
            .or_default()
            .push((uuid, title, path.display().to_string()));
    }

    let mut collisions: Vec<(&String, &Vec<(String, String, String)>)> = by_spec
        .iter()
        .filter(|(_, entries)| entries.len() > 1)
        .collect();
    collisions.sort_by(|a, b| a.0.cmp(b.0));

    if collisions.is_empty() {
        println!(
            "{} no spec_id collisions — every spec_id maps to exactly one requirement.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        return Ok(());
    }

    println!(
        "{} {} spec_id collision(s) found:",
        crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
        collisions.len()
    );
    println!();
    for (spec, entries) in &collisions {
        println!("{}:", spec.bold());
        for (uuid, title, path) in entries.iter() {
            let title_disp = if title.is_empty() {
                "(no title)".dimmed().to_string()
            } else {
                title.clone()
            };
            println!("  {} {} — {}", uuid.yellow(), title_disp, path.dimmed());
        }
        println!();
    }
    println!("Resolution (v1 is detect-only — auto-renumber would orphan trace comments):");
    println!("  - Decide which UUID is canonical for each spec_id");
    println!("  - For the others, edit their YAML directly to set a fresh spec_id, or");
    println!("    delete their YAML if duplicates");
    println!();
    std::process::exit(1);
}

/// The one-line history index freshness summary `aida doctor fsck` prints
/// under cache freshness. Pure, so the wording is testable. `enabled` is
/// whether `AIDA_HISTORY_CACHE` leaves the index switched on.
// trace:TASK-1508 | ai:claude
pub(crate) fn history_index_doctor_line(
    st: &crate::history_cache::HistoryCacheStatus,
    enabled: bool,
) -> String {
    if !enabled {
        return "history index switched off (AIDA_HISTORY_CACHE=0) — `aida history` reads \
                git directly and does not build or update it."
            .to_string();
    }
    let running = if st.indexer_running {
        " (indexing now)"
    } else {
        ""
    };
    if !st.exists {
        return format!(
            "history index not built yet{running} — `aida history` builds it as it goes, \
             or run `aida cache rebuild --history`."
        );
    }
    if let Some(err) = &st.error {
        return format!(
            "history index unreadable ({err}) — `aida history` reads git directly until \
             `aida cache rebuild --history` replaces it."
        );
    }
    let fresh = st.tip.is_some() && st.tip == st.head;
    if !fresh {
        return format!(
            "history index behind the store{running} — it catches up on the next \
             `aida history` query."
        );
    }
    if st.complete {
        format!(
            "history index up to date{running}: {} event(s) covering the whole store history.",
            st.events
        )
    } else {
        let reach = st
            .floor_commit_at
            .as_deref()
            .map(|at| format!(" back to {at}"))
            .unwrap_or_default();
        format!(
            "history index up to date for recent history{reach}{running}; older history \
             is still filling in (run `aida cache rebuild --history` to finish now)."
        )
    }
}

/// Compose every diagnostic into a single report. Exits non-zero on any
// problem so it can gate CI. trace:EPIC-19 | ai:claude
fn doctor_fsck() -> Result<()> {
    let project_root = find_project_root()?;
    let store_path = project_root.join(".aida-store");

    println!("{}", "AIDA fsck".bold());
    println!("  project root:  {}", project_root.display());
    println!("  store path:    {}", store_path.display());
    println!();

    let mut had_problem = false;

    // --- Check 1: block registry consistency (FR-281's logic, inline) ---
    println!("{}", "── block registry ──".bold());
    use aida_core::{BlockRegistry, NodeRegistry};
    let blocks_path = store_path.join("registry").join("blocks.yaml");
    let nodes_path = store_path.join("registry").join("nodes.toml");
    if blocks_path.exists() {
        let blocks = BlockRegistry::load(&blocks_path).unwrap_or_default();
        let nodes = NodeRegistry::load(&nodes_path).unwrap_or_default();
        let registered: std::collections::HashSet<String> =
            nodes.nodes.iter().map(|n| n.id.clone()).collect();
        // Only ACTIVE (non-exhausted) blocks count — tombstoned blocks
        // are explicitly retired (next > range_end) and no longer
        // dispense, so an unregistered owner on a tombstoned block is
        // expected (it's the post-repair state).
        let block_owners: std::collections::HashSet<String> = blocks
            .blocks
            .iter()
            .filter(|b| !b.is_exhausted())
            .map(|b| b.node_id.clone())
            .collect();
        let orphan_blocks: Vec<&str> = block_owners
            .iter()
            .filter(|id| !registered.contains(*id))
            .map(|s| s.as_str())
            .collect();
        if orphan_blocks.is_empty() {
            println!(
                "  {} every active block has a registered node owner.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        } else {
            had_problem = true;
            println!(
                "  {} {} block-owning node(s) not in nodes.toml: {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red(),
                orphan_blocks.len(),
                orphan_blocks.join(", ")
            );
            println!("    fix: {}", "aida doctor repair-stale-blocks".cyan());
        }
    } else {
        println!(
            "  {} no blocks.yaml — skipping (project may be node-aware-only).",
            "·".dimmed()
        );
    }
    println!();

    // --- Check 2: spec_id collisions ---
    println!("{}", "── spec_id collisions ──".bold());
    let objects_root = store_path.join("objects");
    if objects_root.exists() {
        let mut by_spec: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut yaml_files: Vec<std::path::PathBuf> = Vec::new();
        walk_yamls(&objects_root, &mut yaml_files);
        for path in &yaml_files {
            if let Ok(content) = std::fs::read_to_string(path) {
                for line in content.lines() {
                    let t = line.trim_start();
                    if let Some(v) = t.strip_prefix("spec_id:") {
                        let spec = v.trim().trim_matches('"').trim_matches('\'').to_string();
                        if !spec.is_empty() {
                            *by_spec.entry(spec).or_default() += 1;
                            break;
                        }
                    }
                }
            }
        }
        let collisions: Vec<&String> = by_spec
            .iter()
            .filter(|(_, count)| **count > 1)
            .map(|(spec, _)| spec)
            .collect();
        if collisions.is_empty() {
            println!(
                "  {} every spec_id maps to one requirement.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        } else {
            had_problem = true;
            println!(
                "  {} {} spec_id(s) claimed by multiple requirements: {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red(),
                collisions.len(),
                collisions
                    .iter()
                    .take(8)
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            println!("    fix: {}", "aida doctor scrub-collisions".cyan());
        }
    } else {
        println!("  {} no objects/ — skipping.", "·".dimmed());
    }
    println!();

    // --- Check 3: cache freshness ---
    println!("{}", "── cache freshness ──".bold());
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    if cache_path.exists() {
        // Simple heuristic: cache exists. Detailed staleness check
        // requires reading cache HEAD — defer to `aida cache status`.
        println!(
            "  {} cache exists at {} (run `aida cache status` for HEAD-vs-store check).",
            "·".dimmed(),
            cache_path.display()
        );
    } else {
        println!(
            "  {} cache missing — run `aida cache rebuild` if list/search are slow.",
            "·".dimmed()
        );
    }
    // The history index is informational here: a missing, filling or
    // behind index only means slower `aida history` answers, never a
    // wrong one, so it never marks fsck as failed.
    // trace:TASK-1508 | ai:claude
    if store_path.is_dir() {
        println!(
            "  {} {}",
            "·".dimmed(),
            history_index_doctor_line(
                &crate::history_cache::status(&store_path),
                crate::history_cache::cache_enabled_from(
                    std::env::var("AIDA_HISTORY_CACHE").ok().as_deref()
                ),
            )
        );
    }
    println!();

    // --- Check 4: relationship targets resolve ---
    println!("{}", "── relationships ──".bold());
    if objects_root.exists() {
        let mut yaml_files: Vec<std::path::PathBuf> = Vec::new();
        walk_yamls(&objects_root, &mut yaml_files);
        let mut all_uuids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for path in &yaml_files {
            if let Ok(content) = std::fs::read_to_string(path) {
                for line in content.lines() {
                    let t = line.trim_start();
                    if let Some(v) = t.strip_prefix("id:") {
                        let s = v.trim().trim_matches('"').trim_matches('\'').to_string();
                        if !s.is_empty() {
                            all_uuids.insert(s);
                        }
                        break;
                    }
                }
            }
        }
        let mut dangling = 0usize;
        for path in &yaml_files {
            let Ok(content) = std::fs::read_to_string(path) else {
                continue;
            };
            let mut in_rel = false;
            for raw in content.lines() {
                let trimmed = raw.trim_start();
                if !raw.starts_with(' ') && trimmed.starts_with("relationships:") {
                    in_rel = true;
                    continue;
                }
                if in_rel && !raw.starts_with(' ') && !trimmed.is_empty() && trimmed.contains(':') {
                    in_rel = false;
                    continue;
                }
                if !in_rel {
                    continue;
                }
                if let Some(v) = trimmed.strip_prefix("target_id:") {
                    let target = v.trim().trim_matches('"').trim_matches('\'').to_string();
                    if !target.is_empty() && !all_uuids.contains(&target) {
                        dangling += 1;
                    }
                }
            }
        }
        if dangling == 0 {
            println!(
                "  {} every relationship target resolves.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        } else {
            had_problem = true;
            println!(
                "  {} {} dangling relationship reference(s).",
                crate::glyph(crate::glyphs::Glyph::Cross).red(),
                dangling
            );
            println!(
                "    fix: {}",
                "aida doctor verify-relationships --repair".cyan()
            );
        }
    } else {
        println!("  {} no objects/ — skipping.", "·".dimmed());
    }
    println!();

    // --- Check 5: trace comments resolve to existing reqs (informational) ---
    // Slow-ish (walks source tree) and the failure mode (dangling traces
    // from renumbered/deleted reqs) is rarely urgent — keep it
    // non-blocking so fsck can serve as a CI gate without a perpetual
    // false-fail. trace:EPIC-19 | ai:claude
    println!("{}", "── trace comments ──".bold());
    if objects_root.exists() {
        let trace_re =
            regex::Regex::new(r"trace:([A-Z]+(?:-[A-Z0-9]+)?-[0-9]+(?:-[0-9]+)?)").unwrap();
        let mut yaml_files: Vec<std::path::PathBuf> = Vec::new();
        walk_yamls(&objects_root, &mut yaml_files);
        let mut known_specs: std::collections::HashSet<String> = std::collections::HashSet::new();
        for path in &yaml_files {
            if let Ok(content) = std::fs::read_to_string(path) {
                for line in content.lines() {
                    let t = line.trim_start();
                    if let Some(v) = t.strip_prefix("spec_id:") {
                        let s = v.trim().trim_matches('"').trim_matches('\'').to_string();
                        if !s.is_empty() {
                            known_specs.insert(s);
                        }
                    } else if let Some(v) = t.strip_prefix("agreed_id:") {
                        let s = v.trim().trim_matches('"').trim_matches('\'').to_string();
                        if !s.is_empty() && s != "null" && s != "~" {
                            known_specs.insert(s);
                        }
                    }
                }
            }
        }
        let mut by_spec: std::collections::HashMap<String, Vec<(std::path::PathBuf, usize)>> =
            std::collections::HashMap::new();
        walk_source_for_traces(&project_root, &trace_re, &mut by_spec);
        let total_refs: usize = by_spec.values().map(|v| v.len()).sum();
        let dangling: usize = by_spec
            .iter()
            .filter(|(s, _)| !known_specs.contains(*s))
            .map(|(_, v)| v.len())
            .sum();
        let dangling_specs: usize = by_spec.keys().filter(|s| !known_specs.contains(*s)).count();
        if dangling == 0 {
            println!(
                "  {} {} unique spec_ids referenced from {} location(s); all resolve.",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                by_spec.len(),
                total_refs
            );
        } else {
            // Informational, not a failure — see comment above.
            println!(
                "  {} {} unique spec_ids referenced from {} location(s); {} reference(s) ({} unique spec_ids) dangling.",
                "·".yellow(),
                by_spec.len(),
                total_refs,
                dangling,
                dangling_specs
            );
            println!(
                "    detail: {}",
                "aida doctor validate-trace-comments".cyan()
            );
        }
    } else {
        println!("  {} no objects/ — skipping.", "·".dimmed());
    }
    println!();

    // --- Check 6: counter_scope sanity (warn if config + blocks disagree) ---
    println!("{}", "── counter_scope ──".bold());
    let scope = read_id_counter_scope(&project_root);
    let has_global_block = blocks_path.exists()
        && BlockRegistry::load(&blocks_path)
            .map(|br| {
                br.blocks
                    .iter()
                    .any(|b| b.type_prefix == aida_core::IdCounterScope::GLOBAL_TYPE_PREFIX)
            })
            .unwrap_or(false);
    match (scope, has_global_block) {
        (aida_core::IdCounterScope::Global, true) => {
            println!(
                "  {} config=global, blocks have a `*` block. Consistent.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        }
        (aida_core::IdCounterScope::Global, false) => {
            had_problem = true;
            println!(
                "  {} config says global but no `*` block exists. New `aida add` would fall back to per-type.",
                crate::glyph(crate::glyphs::Glyph::Cross).red()
            );
            println!(
                "    fix: {}",
                "aida doctor migrate-counter-scope --to global".cyan()
            );
        }
        (aida_core::IdCounterScope::PerType, true) => {
            println!(
                "  {} config=per-type, but a `*` block exists (mid-migration?). Consider running `migrate-counter-scope --to global`.",
                "·".yellow()
            );
        }
        (aida_core::IdCounterScope::PerType, false) => {
            println!(
                "  {} config=per-type, no `*` block. Consistent.",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
        }
    }
    println!();

    // --- Check 7: legacy-store cruft on a git-canonical project ---
    // TASK-752: tracked requirements*.yaml / scaffold-report.html while the live
    // store is the orphan aida-store branch. The detector self-gates on
    // distributed mode + orphan branch, so this is silent on a legacy
    // --centralized project. A finding is a problem (non-zero exit) so fsck can
    // gate CI; `aida doctor --heal --category legacy-store-cruft` resolves it.
    // trace:TASK-752 | ai:claude
    println!("{}", "── legacy-store cruft ──".bold());
    let cruft = detect_legacy_store_cruft(&project_root);
    if cruft.is_empty() {
        println!(
            "  {} no tracked legacy-store artifacts (or not a git-canonical project).",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    } else {
        had_problem = true;
        println!(
            "  {} {} tracked legacy-store artifact(s) on a git-canonical project: {}",
            crate::glyph(crate::glyphs::Glyph::Cross).red(),
            cruft.len(),
            cruft.join(", ")
        );
        println!(
            "    fix: {}",
            "aida doctor --heal --category legacy-store-cruft".cyan()
        );
    }
    println!();

    // --- Check 8: per-clone runtime files tracked on the orphan aida-store branch ---
    // BUG-563: `.aida/node.toml` / dispenser.toml / *.lock / cache.db* tracked on
    // the orphan branch make every cross-clone store-leg rebase conflict forever.
    // The detector self-gates on distributed mode + an attached store worktree.
    // A finding is a problem (non-zero exit) so fsck can gate CI;
    // `aida doctor --heal --category store-tracked-runtime` resolves it.
    // trace:BUG-563 | ai:claude
    println!("{}", "── store-tracked runtime ──".bold());
    let runtime_cruft = detect_store_tracked_runtime(&project_root);
    if runtime_cruft.is_empty() {
        println!(
            "  {} no per-clone runtime files tracked on the orphan aida-store branch (or not a git-canonical project).",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
    } else {
        had_problem = true;
        println!(
            "  {} {} per-clone runtime file(s) tracked on the orphan aida-store branch: {}",
            crate::glyph(crate::glyphs::Glyph::Cross).red(),
            runtime_cruft.len(),
            runtime_cruft.join(", ")
        );
        println!(
            "    fix: {}",
            "aida doctor --heal --category store-tracked-runtime".cyan()
        );
    }
    println!();

    if had_problem {
        println!("{}", "fsck found problems — see above.".red().bold());
        std::process::exit(1);
    } else {
        println!(
            "{}",
            format!("{} fsck clean.", crate::glyph(crate::glyphs::Glyph::Check))
                .green()
                .bold()
        );
    }
    Ok(())
}

fn doctor_migrate_counter_scope(
    to: &str,
    dry_run: bool,
    yes: bool,
    new_block_size: u32,
) -> Result<()> {
    use aida_core::BlockRegistry;

    if to != "global" {
        anyhow::bail!("only `--to global` is supported today (per-type → global)");
    }

    let project_root = find_project_root()?;
    let store_path = project_root.join(".aida-store");
    let blocks_path = store_path.join("registry").join("blocks.yaml");
    let config_path = project_root.join(".aida").join("config.toml");

    if !blocks_path.exists() {
        anyhow::bail!(
            "no blocks.yaml at {} — nothing to migrate",
            blocks_path.display()
        );
    }
    if !config_path.exists() {
        anyhow::bail!(
            "no config.toml at {} — is this an AIDA project?",
            config_path.display()
        );
    }

    let current_scope = read_id_counter_scope(&project_root);
    if current_scope == aida_core::IdCounterScope::Global {
        println!(
            "{} already on global counter_scope — nothing to migrate.",
            crate::glyph(crate::glyphs::Glyph::Check).green()
        );
        return Ok(());
    }

    let mut registry = BlockRegistry::load(&blocks_path)?;
    if registry.blocks.is_empty() {
        anyhow::bail!("blocks.yaml is empty — no blocks to migrate from");
    }

    // Identify this clone's node id so the new `*` block belongs to it.
    let node_id = load_node_id(&store_path);
    let our_blocks: Vec<_> = registry
        .blocks
        .iter()
        .filter(|b| b.node_id == node_id && !b.is_exhausted())
        .cloned()
        .collect();
    if our_blocks.is_empty() {
        anyhow::bail!(
            "node {} has no active per-type blocks in blocks.yaml — \
             either already migrated, or this clone hasn't been initialized",
            node_id
        );
    }

    // The new `*` block starts strictly above the highest range_end across
    // ALL blocks (any node, any type) so we never collide with another
    // clone's range. Then size more on top.
    let highest_end: u32 = registry
        .blocks
        .iter()
        .map(|b| b.range_end)
        .max()
        .unwrap_or(0);
    let new_start = highest_end + 1;
    let new_end = new_start + new_block_size - 1;

    println!("{}", "Migration plan: per-type → global".bold());
    println!("  node:                {}", node_id);
    println!("  per-type blocks to retire (mark exhausted):");
    for b in &our_blocks {
        println!(
            "    - {} {}-{}..{} (next was {})",
            "·".dimmed(),
            b.type_prefix,
            b.range_start,
            b.range_end,
            b.next
        );
    }
    println!(
        "  new global block:    *-{}..{} (size {}) for node {}",
        new_start, new_end, new_block_size, node_id
    );
    println!("  config write:        [id_format] counter_scope = \"global\"");
    println!();
    println!("After this migration:");
    println!("  - existing requirement spec_ids stay UNCHANGED");
    println!(
        "  - new requirements use the global counter (FR-{}, BUG-{}, etc.)",
        new_start,
        new_start + 1
    );
    println!("  - the retired per-type blocks remain in blocks.yaml as history");
    println!();

    if dry_run {
        println!("{} dry-run — no changes written.", "→".cyan());
        return Ok(());
    }

    if !yes {
        use std::io::Write;
        print!("Proceed? [y/N] ");
        std::io::stdout().flush()?;
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Apply: mark our per-type blocks exhausted (next = range_end + 1) so
    // the dispenser skips them. Then append the new `*` block.
    for b in registry.blocks.iter_mut() {
        if b.node_id == node_id && !b.is_exhausted() {
            b.next = b.range_end + 1;
        }
    }
    let owner = aida_core::git_ops::git_config_get("user.email")
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "unknown".to_string());
    registry.claim_block_with_floor(
        node_id.clone(),
        owner,
        hostname(),
        aida_core::IdCounterScope::GLOBAL_TYPE_PREFIX.to_string(),
        new_block_size,
        highest_end,
    );
    registry.save(&blocks_path)?;

    // Update config.toml — preserve the file by line-rewriting; if
    // counter_scope already exists (it shouldn't given the early check),
    // overwrite. Otherwise append after the [id_format] section.
    update_config_counter_scope(&config_path, "global")?;

    // Stage + commit the registry change. The lease symlink in the
    // session worktree means `git -C <store_path>` operates on the
    // shared orphan branch.
    let _ = aida_core::git_ops::add(&store_path, &["registry/blocks.yaml"]);
    let _ = aida_core::git_ops::commit(
        &store_path,
        &format!(
            "chore(registry): migrate node {} to global counter (*-{}..{})",
            node_id, new_start, new_end
        ),
    );

    println!();
    println!(
        "{} migration complete.",
        crate::glyph(crate::glyphs::Glyph::Check).green().bold()
    );
    println!(
        "  new global block: {}",
        format!("*-{}..{}", new_start, new_end).cyan()
    );
    println!(
        "  next `aida add` will dispense {}",
        format!("<TYPE>-{}", new_start).cyan()
    );
    println!();
    println!("Don't forget to push:");
    println!("  {}", "aida push".cyan());
    Ok(())
}

/// Update the `[id_format] counter_scope` value in config.toml. Adds the
/// line if missing, replaces it in-place if present. Preserves the rest
/// of the file (comments, other keys, formatting).
fn update_config_counter_scope(config_path: &std::path::Path, new_value: &str) -> Result<()> {
    let content = std::fs::read_to_string(config_path)?;
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let mut in_id_format = false;
    let mut last_id_format_line: Option<usize> = None;
    let mut replaced = false;
    for (i, line) in lines.iter_mut().enumerate() {
        let trimmed_owned: String = line.trim().to_string();
        if trimmed_owned.starts_with('[') {
            in_id_format = trimmed_owned == "[id_format]";
            if in_id_format {
                last_id_format_line = Some(i);
            }
            continue;
        }
        if in_id_format && trimmed_owned.starts_with("counter_scope") {
            *line = format!("counter_scope = \"{}\"", new_value);
            replaced = true;
        }
        if in_id_format && !trimmed_owned.is_empty() && !trimmed_owned.starts_with('#') {
            last_id_format_line = Some(i);
        }
    }
    if !replaced {
        // Insert after the last line of the [id_format] section.
        let insert_at = last_id_format_line.map(|i| i + 1);
        let new_line = format!("counter_scope = \"{}\"", new_value);
        match insert_at {
            Some(idx) => lines.insert(idx, new_line),
            None => {
                // No [id_format] section found — append both header and value.
                lines.push(String::new());
                lines.push("[id_format]".to_string());
                lines.push(new_line);
            }
        }
    }
    std::fs::write(config_path, lines.join("\n") + "\n")?;
    Ok(())
}

/// Sweep the requirement store for semantic contradictions (STORY-1426).
// trace:STORY-1426 | ai:antigravity
pub(crate) fn doctor_contradictions(
    json: bool,
    limit: usize,
    offset: usize,
    all: bool,
) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let store = load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "Failed to load requirements store from {}",
            project_root.display()
        )
    })?;

    let jev = crate::evaluator::JevEvaluator::from_env().ok();
    let findings = crate::contradictions::sweep_contradictions_at(
        &store,
        jev.as_ref()
            .map(|j| j as &dyn crate::evaluator::EvaluatorEngine),
        &project_root,
    )?;

    let page = crate::contradictions::paginate_findings(findings, limit, offset, all);

    if json {
        println!("{}", serde_json::to_string_pretty(&page)?);
    } else {
        crate::contradictions::render_findings_page(
            &page.findings,
            page.total,
            page.offset,
            page.limit,
            &page.category_counts,
        );
    }

    if page.total > 0 {
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod bug_1505_review_verdict_doctor_tests {
    use super::*;

    // trace:BUG-1505 | ai:claude
    #[test]
    fn unknown_verdicts_get_their_own_finding_and_drift_is_aggregated() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("PR-1979-advisor.json"),
            r#"{"verdict":"CONTENT APPROVED — MERGE WITHHELD FOR INDEPENDENCE","summary":"s","head":"3ae8f937"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("PR-5.json"),
            r#"{"verdict":"APPROVED","summary":"s"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("PR-6.json"),
            r#"{"verdict":"approved","summary":"s","reviewed_sha":"aaaaaaa","recorded_by":"t"}"#,
        )
        .unwrap();
        let findings = scan_review_verdicts(tmp.path());
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(findings
            .iter()
            .any(|f| f.id == "unknown-verdict:PR-1979-advisor.json"));
        let agg = findings
            .iter()
            .find(|f| f.id == "non-canonical-verdicts")
            .unwrap();
        assert!(
            agg.summary.starts_with("1 review-verdict file"),
            "{}",
            agg.summary
        );
        assert!(agg.summary.contains("PR-5.json"));
        assert_eq!(
            normalize_doctor_category("verdicts").unwrap(),
            "review-verdicts"
        );
    }
}
