//! `aida plan` command cluster — verify / helpers / promote / fan-out /
//! capture / scan (TASK-92 / TASK-93 / TASK-94 / TASK-305 / STORY-519 /
//! TASK-0418).
//!
//! Extracted verbatim from `main.rs` (SPIKE-78, pure movement; no behavior
//! change). The shared plan-verify/lint *engine* (`compute_plan_report`,
//! `plan_repo_root`, `render_plan_report_string`, and the `PlanReport` /
//! `PlanFinding` / `PlanFindingLevel` / `PlanRefFix` / `PlanSectionSpec`
//! types) stays in `main.rs` because `aida skill lint` and the MCP
//! `plan_verify` tool also depend on it; this module reaches it via `crate::`.
//! `build_reusable_helpers_section` likewise stays in `main.rs` (shared with
//! the `aida ultraplan` prompt assembler).

use crate::*;

pub(crate) fn handle_plan_command(cmd: &PlanCommand) -> Result<()> {
    match cmd {
        PlanCommand::Verify { file, fix, quiet } => verify_plan(file, *fix, *quiet),
        PlanCommand::Helpers { spec, append } => plan_helpers(spec, append.as_deref()),
        PlanCommand::Promote { spec, all, dry_run } => {
            plan_promote(spec.as_deref(), *all, *dry_run)
        }
        PlanCommand::FanOut {
            specs,
            batch,
            epic,
            include_low,
            dry_run,
            promote_only,
        } => plan_fan_out(
            specs,
            batch.as_deref(),
            epic.as_deref(),
            *include_low,
            *dry_run,
            *promote_only,
        ),
        PlanCommand::Capture { pr, stdout } => plan_capture(pr, *stdout),
        PlanCommand::Scan {
            spec,
            attach,
            append,
            json,
        } => plan_scan(spec, *attach, append.as_deref(), *json),
    }
}

/// STORY-265: the SPEC-IDs listed on a plan file's `Specs:` header line
/// (`Specs: STORY-N, BUG-M`). Comma- or whitespace-separated; only the first
/// `Specs:` line (the header) is read.
// trace:STORY-265 | ai:claude
fn parse_plan_specs(content: &str) -> Vec<String> {
    for line in content.lines() {
        let t = line.trim();
        let rest = t
            .strip_prefix("Specs:")
            .or_else(|| t.strip_prefix("specs:"));
        if let Some(rest) = rest {
            return rest
                .split([',', ' ', '\t'])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
    }
    Vec::new()
}

/// STORY-265: find the plan file under `plans_dir` whose `Specs:` header lists
/// `spec_id` (case-insensitive). Skips `_`-prefixed files (e.g. `_TEMPLATE.md`)
/// and only scans the header region. Returns the lexicographically-last match
/// so the newest date-prefixed plan wins.
// trace:STORY-265 | ai:claude
fn find_plan_file_for_spec(
    plans_dir: &std::path::Path,
    spec_id: &str,
) -> Option<std::path::PathBuf> {
    let mut matches: Vec<std::path::PathBuf> = Vec::new();
    for entry in std::fs::read_dir(plans_dir).ok()?.flatten() {
        let p = entry.path();
        if p.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        let name = p.file_name().and_then(|x| x.to_str()).unwrap_or("");
        if name.starts_with('_') {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&p) else {
            continue;
        };
        let head: String = content.lines().take(30).collect::<Vec<_>>().join("\n");
        if parse_plan_specs(&head)
            .iter()
            .any(|s| s.eq_ignore_ascii_case(spec_id))
        {
            matches.push(p);
        }
    }
    matches.sort();
    matches.pop()
}

// STORY-265: promote Approved spec(s) to Planned when a plan file exists.
// Reads the store read-only to find eligible specs (status Approved AND a
// `docs/plans/` file listing the SPEC-ID), then performs the transition via
// the proven `aida edit --status planned` path (history + write-through +
// the ADR-3 authority gate). trace:STORY-265 | ai:claude
// ============================================================================
// `aida plan capture <PR>` — synthesize a docs/plans/ file from a PR's
// description + commit log. For plans authored via the web `/ultraplan` flow
// that land a PR directly, leaving no local plan file (TASK-305). The PR
// description carries the plan summary; the commit log is the step-by-step
// execution; `gh pr diff --name-only` is the blast radius. We fill the
// 11-section template so the captured file passes `aida plan verify`.
// trace:TASK-305 | ai:claude
// ============================================================================

/// The structured input `synthesize_plan_from_pr` needs, kept separate from the
/// `gh` subprocess calls so the synthesis is a pure function (PR data → plan
/// markdown) that can be unit-tested with a fixture.
// trace:TASK-305 | ai:claude
#[derive(Debug, Clone, Default)]
struct CapturedPr {
    number: u64,
    title: String,
    body: String,
    /// Commit subjects (message headlines), in chronological order.
    commit_subjects: Vec<String>,
    /// Files changed, from `gh pr diff --name-only`.
    changed_files: Vec<String>,
}

/// Parse a PR argument that may arrive as `65`, `PR-65`, `pr-65`, or `#65`.
// trace:TASK-305 | ai:claude
fn parse_pr_number(arg: &str) -> Result<u64> {
    let trimmed = arg.trim();
    let digits = trimmed
        .trim_start_matches('#')
        .trim_start_matches("PR-")
        .trim_start_matches("pr-")
        .trim_start_matches("Pr-")
        .trim();
    digits.parse::<u64>().map_err(|_| {
        anyhow::anyhow!("could not parse `{arg}` as a PR number (try a bare number like `65`)")
    })
}

/// Scan free text for lines that look like test / verification commands so the
/// captured plan's `## Verification` section is grounded in what the PR author
/// actually claimed they ran. Matches fenced-code lines and inline `cargo` /
/// `aida` / `npm` / `make` / `pytest` / `go test` invocations. Returns the
/// matched lines verbatim (deduped, order-preserving).
// trace:TASK-305 | ai:claude
fn extract_verification_commands(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut in_fence = false;
    for raw in body.lines() {
        let line = raw.trim();
        if line.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        // Strip a leading list marker / `$` prompt so the heuristic sees the command.
        let candidate = line
            .trim_start_matches("- ")
            .trim_start_matches("* ")
            .trim_start_matches("$ ")
            .trim();
        let looks_like_cmd = candidate.starts_with("cargo ")
            || candidate.starts_with("aida ")
            || candidate.starts_with("npm ")
            || candidate.starts_with("pnpm ")
            || candidate.starts_with("yarn ")
            || candidate.starts_with("make ")
            || candidate.starts_with("pytest")
            || candidate.starts_with("go test")
            || candidate.starts_with("./")
            || candidate.starts_with("bash ");
        // trace:BUG-473 | ai:claude
        // Inside a fenced block we keep any command-shaped line; outside, only
        // ones starting with a recognised tool so prose doesn't leak in. The
        // in-fence path is intentionally looser than the tool-prefix list: a
        // ```bash block is already an explicit command region, so we keep any
        // non-empty, non-comment line (e.g. `RUSTFLAGS=… cargo build`, `cd x &&
        // make`) that the tool-prefix heuristic would otherwise drop. (The old
        // condition `(in_fence && looks_like_cmd) || looks_like_cmd` collapsed to
        // `looks_like_cmd`, so `in_fence` had no effect and this looser capture
        // never happened — BUG-473.)
        let in_fence_cmd = !candidate.is_empty() && !candidate.starts_with('#');
        if ((in_fence && in_fence_cmd) || looks_like_cmd)
            && !candidate.is_empty()
            && !out.iter().any(|c| c == candidate)
        {
            out.push(candidate.to_string());
        }
    }
    out
}

/// Synthesize the 11-section plan markdown from captured PR data. Pure function
/// (no I/O) so it is unit-tested against a fixture. `date` is injected so the
/// header is deterministic in tests. Missing-data sections carry an explicit
/// `<!-- not captured from PR -->` marker rather than being omitted, so the
/// output still passes `aida plan verify` (which treats Critical Files /
/// Verification / Followups as hard-required).
// trace:TASK-305 | ai:claude
fn synthesize_plan_from_pr(pr: &CapturedPr, date: &str) -> String {
    const NOT_CAPTURED: &str = "<!-- not captured from PR -->";

    // Spec ids credited across the title + every commit subject — for the
    // header `Specs:` line. First-seen order, deduped case-insensitively.
    let mut specs: Vec<String> = Vec::new();
    let push_specs = |text: &str, acc: &mut Vec<String>| {
        for id in extract_spec_ids_from_commit(text) {
            if !acc.iter().any(|x| x.eq_ignore_ascii_case(&id)) {
                acc.push(id);
            }
        }
    };
    push_specs(&pr.title, &mut specs);
    for subj in &pr.commit_subjects {
        push_specs(subj, &mut specs);
    }
    let specs_line = if specs.is_empty() {
        NOT_CAPTURED.to_string()
    } else {
        specs.join(", ")
    };

    let title = if pr.title.trim().is_empty() {
        format!("PR-{}", pr.number)
    } else {
        pr.title.trim().to_string()
    };

    let body = pr.body.trim();
    let approach = if body.is_empty() {
        format!(
            "{NOT_CAPTURED}\n\nThis plan was reconstructed from PR-{} after the fact; the PR \
             carried no description.",
            pr.number
        )
    } else {
        body.to_string()
    };

    let mut md = String::new();

    // ── Header block (the 11th "section": the metadata preamble). ──
    md.push_str(&format!("# Plan: {title}\n\n"));
    md.push_str(&format!("Date: {date}\n"));
    md.push_str(&format!("Specs: {specs_line}\n"));
    md.push_str("Status: Completed\n");
    md.push_str(&format!(
        "Complexity: {} commits, {} files changed (reconstructed from PR)\n",
        pr.commit_subjects.len(),
        pr.changed_files.len()
    ));
    md.push_str(&format!("Source: web /ultraplan PR-{}\n\n", pr.number));
    md.push_str(&format!(
        "<!--\n  Captured by `aida plan capture {}` from the PR description + commit log.\n  \
         The web /ultraplan flow lands a PR directly without writing a local plan file;\n  \
         this reconstructs the AIDA plan-archival record after the fact. trace:TASK-305\n-->\n\n",
        pr.number
    ));

    // ── Approach (from PR description). ──
    md.push_str("## Approach\n\n");
    md.push_str(&approach);
    md.push_str("\n\n");

    // ── Decisions. Not separable from a free-form PR body. ──
    md.push_str("## Decisions\n\n");
    md.push_str(&format!(
        "{NOT_CAPTURED} — decisions are not separable from the PR description above. See the \
         `## Approach` section (the PR body) for the rationale the author recorded.\n\n"
    ));

    // ── Files (in build-order) — the commit log is the step-by-step execution. ──
    md.push_str("## Files (in build-order)\n\n");
    if pr.commit_subjects.is_empty() {
        md.push_str(&format!("{NOT_CAPTURED} — no commits found on the PR.\n\n"));
    } else {
        md.push_str(
            "Reconstructed from the PR commit log (chronological — each commit is one step):\n\n",
        );
        for (i, subj) in pr.commit_subjects.iter().enumerate() {
            md.push_str(&format!("{}. {}\n", i + 1, subj.trim()));
        }
        md.push('\n');
    }

    // ── Critical Files (hard-required) — from `gh pr diff --name-only`. ──
    md.push_str("## Critical Files\n\n");
    if pr.changed_files.is_empty() {
        md.push_str(&format!(
            "{NOT_CAPTURED} — no changed files reported for the PR.\n\n"
        ));
    } else {
        for f in &pr.changed_files {
            md.push_str(&format!("- `{}`\n", f.trim()));
        }
        md.push('\n');
    }

    // ── Reusable helpers. Cannot be derived from a PR after the fact. ──
    md.push_str("## Reusable helpers (do not reimplement)\n\n");
    md.push_str(&format!(
        "{NOT_CAPTURED} — reusable-helper analysis runs from the trace graph at plan time. \
         Run `aida plan helpers <SPEC>` if you need it for follow-up work.\n\n"
    ));

    // ── Risks + gotchas. ──
    md.push_str("## Risks + gotchas\n\n");
    md.push_str(&format!(
        "{NOT_CAPTURED} — risks were not recorded separately from the PR description.\n\n"
    ));

    // ── Tests (named). ──
    md.push_str("## Tests\n\n");
    md.push_str(&format!(
        "{NOT_CAPTURED} — see the `## Verification` section for any test commands the PR \
         description mentioned.\n\n"
    ));

    // ── Verification (hard-required) — mine the PR body for command-shaped lines. ──
    md.push_str("## Verification\n\n");
    let verif = extract_verification_commands(body);
    if verif.is_empty() {
        md.push_str(&format!(
            "{NOT_CAPTURED} — the PR description named no test/verification commands.\n\n"
        ));
    } else {
        md.push_str("Commands the PR description named as verification:\n\n");
        md.push_str("```bash\n");
        for c in &verif {
            md.push_str(c);
            md.push('\n');
        }
        md.push_str("```\n\n");
    }

    // ── Followups (hard-required). ──
    md.push_str("## Followups\n\n");
    md.push_str(&format!(
        "{NOT_CAPTURED} — no out-of-scope followups were captured from the PR.\n\n"
    ));

    // ── Related. ──
    md.push_str("## Related\n\n");
    if specs.is_empty() {
        md.push_str(&format!("- Source: web /ultraplan PR-{}\n", pr.number));
    } else {
        md.push_str(&format!("- Specs: {}\n", specs.join(", ")));
        md.push_str(&format!("- Source: web /ultraplan PR-{}\n", pr.number));
    }

    md
}

/// Build the output filename slug for a captured plan: prefer the first
/// credited spec id, else a slug of the PR title, else `pr-<N>`. Always
/// suffixed with `-from-pr-<N>` so the provenance is in the path and the file
/// is deterministic (idempotent re-capture overwrites the same path).
// trace:TASK-305 | ai:claude
fn captured_plan_slug(pr: &CapturedPr) -> String {
    let mut specs: Vec<String> = Vec::new();
    for id in extract_spec_ids_from_commit(&pr.title) {
        specs.push(id);
    }
    if specs.is_empty() {
        // trace:BUG-473 | ai:claude
        // Take the first spec id from the first commit subject that yields one.
        // (Was a loop-once `for id in … { push; break; }`; `.into_iter().next()`
        // expresses the same intent without the never-loop construct.)
        for subj in &pr.commit_subjects {
            if let Some(id) = extract_spec_ids_from_commit(subj).into_iter().next() {
                specs.push(id);
                break;
            }
        }
    }
    let base = if let Some(first) = specs.first() {
        slugify_str(first)
    } else if !pr.title.trim().is_empty() {
        let s = slugify_str(&pr.title);
        // Bound the slug length so filenames stay sane.
        s.split('-').take(6).collect::<Vec<_>>().join("-")
    } else {
        String::new()
    };
    if base.is_empty() {
        format!("pr-{}", pr.number)
    } else {
        format!("{base}-from-pr-{}", pr.number)
    }
}

/// Fetch a PR's title/body/commit-subjects via `gh pr view --json` and the
/// changed files via `gh pr diff --name-only`. Isolated from the synthesis so
/// the pure function stays testable.
// trace:TASK-305 | ai:claude
fn fetch_captured_pr(project_root: &std::path::Path, number: u64) -> Result<CapturedPr> {
    let gh = resolve_gh_binary().ok_or_else(|| {
        anyhow::anyhow!("`gh` not on PATH — install from https://cli.github.com/")
    })?;
    let n_str = number.to_string();

    let view = std::process::Command::new(&gh)
        .current_dir(project_root)
        .args(["pr", "view", &n_str, "--json", "number,title,body,commits"])
        .output()
        .with_context(|| format!("`gh pr view {number}` failed to spawn"))?;
    if !view.status.success() {
        anyhow::bail!(
            "`gh pr view {}` exited {} — {}",
            number,
            view.status,
            String::from_utf8_lossy(&view.stderr).trim()
        );
    }
    let json: serde_json::Value = serde_json::from_slice(&view.stdout)
        .with_context(|| format!("`gh pr view {number}` returned non-JSON output"))?;

    let title = json
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let body = json
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let commit_subjects: Vec<String> = json
        .get("commits")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    c.get("messageHeadline")
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })
                .collect()
        })
        .unwrap_or_default();

    // Changed files. A diff failure (e.g. the PR is closed and the branch is
    // gone) is non-fatal — Critical Files just falls back to not-captured.
    let changed_files: Vec<String> = std::process::Command::new(&gh)
        .current_dir(project_root)
        .args(["pr", "diff", &n_str, "--name-only"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default();

    Ok(CapturedPr {
        number,
        title,
        body,
        commit_subjects,
        changed_files,
    })
}

/// `aida plan capture <PR>` handler.
// trace:TASK-305 | ai:claude
fn plan_capture(pr_arg: &str, stdout: bool) -> Result<()> {
    use colored::Colorize;
    let number = parse_pr_number(pr_arg)?;
    let project_root = find_project_root().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let captured = fetch_captured_pr(&project_root, number)?;
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let md = synthesize_plan_from_pr(&captured, &date);

    if stdout {
        print!("{md}");
        return Ok(());
    }

    let plans_dir = project_root.join("docs").join("plans");
    std::fs::create_dir_all(&plans_dir)
        .with_context(|| format!("could not create {}", plans_dir.display()))?;
    let slug = captured_plan_slug(&captured);
    let filename = format!("{date}-{slug}.md");
    let path = plans_dir.join(&filename);
    let existed = path.exists();
    std::fs::write(&path, &md).with_context(|| format!("could not write {}", path.display()))?;

    let verb = if existed { "overwrote" } else { "wrote" };
    println!(
        "{} {} {} from PR-{}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        verb,
        path.display().to_string().bold(),
        number
    );
    println!(
        "  {}",
        "review + edit the synthesized sections, then `aida plan verify` it".dimmed()
    );
    Ok(())
}

fn plan_promote(spec: Option<&str>, all: bool, dry_run: bool) -> Result<()> {
    use colored::Colorize;
    let project_root = find_project_root().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let plans_dir = project_root.join("docs").join("plans");
    let store = load_store_for_lookup(&project_root)
        .ok_or_else(|| anyhow::anyhow!("could not load the requirement store"))?;

    let targets: Vec<String> = if let Some(s) = spec {
        vec![s.to_string()]
    } else if all {
        store
            .requirements
            .iter()
            .filter(|r| r.status == aida_core::RequirementStatus::Approved)
            .filter_map(|r| r.spec_id.clone())
            .collect()
    } else {
        anyhow::bail!("specify a SPEC-ID to promote, or pass --all to sweep every Approved spec");
    };

    let (mut eligible, mut promoted, mut skipped) = (0usize, 0usize, 0usize);
    for sid in targets {
        let Some(req) = store.requirements.iter().find(|r| {
            r.spec_id
                .as_deref()
                .map(|x| x.eq_ignore_ascii_case(&sid))
                .unwrap_or(false)
        }) else {
            eprintln!(
                "  {} {} not found",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                sid
            );
            continue;
        };
        let real_id = req.spec_id.clone().unwrap_or(sid);
        if req.status != aida_core::RequirementStatus::Approved {
            // Only narrate skips for an explicitly-named spec, not the --all sweep.
            if spec.is_some() {
                println!(
                    "  {} {} is {:?}, not Approved — skipped",
                    "↷".dimmed(),
                    real_id,
                    req.status
                );
            }
            skipped += 1;
            continue;
        }
        match find_plan_file_for_spec(&plans_dir, &real_id) {
            Some(plan) => {
                eligible += 1;
                let rel = plan.strip_prefix(&project_root).unwrap_or(&plan);
                if dry_run {
                    println!(
                        "  {} {} → Planned (plan: {})",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        real_id,
                        rel.display()
                    );
                } else {
                    let status = std::process::Command::new(std::env::current_exe()?)
                        .args(["edit", &real_id, "--status", "planned"])
                        .status()?;
                    if status.success() {
                        println!(
                            "  {} {} → Planned (plan: {})",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            real_id,
                            rel.display()
                        );
                        promoted += 1;
                    } else {
                        eprintln!(
                            "  {} {} — status edit failed",
                            crate::glyph(crate::glyphs::Glyph::Cross).red(),
                            real_id
                        );
                    }
                }
            }
            None => {
                if spec.is_some() {
                    println!(
                        "  {} {} has no plan file in {} (a plan's `Specs:` header must list {}) — skipped",
                        "↷".dimmed(),
                        real_id,
                        plans_dir.display(),
                        real_id
                    );
                }
                skipped += 1;
            }
        }
    }

    if dry_run {
        println!("(dry-run) {eligible} eligible for promotion, {skipped} skipped");
    } else {
        println!("{promoted} promoted to Planned, {skipped} skipped");
    }
    Ok(())
}

/// A spec considered for plan-only fan-out, reduced to the fields the
/// selector reasons over. Keeping this a plain struct (not the full
/// `Requirement`) lets the selection logic be unit-tested with no store.
// trace:STORY-519 | ai:claude
#[derive(Debug, Clone)]
struct FanOutCandidate {
    spec_id: String,
    is_approved: bool,
    is_low_priority: bool,
    tags: Vec<String>,
}

/// How the fan-out set was selected. Drives both resolution and the
/// human-readable header.
// trace:STORY-519 | ai:claude
enum FanOutSelector<'a> {
    /// Explicit SPEC-IDs passed on the command line.
    Specs(&'a [String]),
    /// Every Approved spec tagged `batch:NAME`.
    Batch(&'a str),
    /// Every Approved spec tagged `parent:ID`.
    Epic(&'a str),
}

/// Pure resolution + workable-set filtering for plan-only fan-out.
///
/// Returns the ordered set of SPEC-IDs to fan out over, plus the SPEC-IDs
/// that were dropped by the workable-set discipline (low-priority tail,
/// or non-Approved status) so the caller can narrate them. The selector
/// determines membership; the workable-set rules then prune:
///   - non-Approved specs are always excluded (planning is an
///     Approved -> Planned pre-step; nothing else qualifies);
///   - low-priority specs are excluded unless `include_low` is set.
///
/// For an explicit `Specs` selection the requested ids are kept in the
/// order given; for `Batch`/`Epic` the matching candidates are returned
/// in their natural store order. Unknown explicit ids are reported as
/// dropped (caller surfaces "not found").
// trace:STORY-519 | ai:claude
fn resolve_fan_out_set(
    candidates: &[FanOutCandidate],
    selector: &FanOutSelector,
    include_low: bool,
) -> (Vec<String>, Vec<String>) {
    let tag_matches =
        |c: &FanOutCandidate, want: &str| c.tags.iter().any(|t| t.eq_ignore_ascii_case(want));

    // First pick the membership set in a stable order.
    let members: Vec<&FanOutCandidate> = match selector {
        FanOutSelector::Specs(ids) => {
            let mut out = Vec::new();
            for id in *ids {
                if let Some(c) = candidates
                    .iter()
                    .find(|c| c.spec_id.eq_ignore_ascii_case(id))
                {
                    out.push(c);
                }
            }
            out
        }
        FanOutSelector::Batch(name) => {
            let want = format!("batch:{name}");
            candidates
                .iter()
                .filter(|c| tag_matches(c, &want))
                .collect()
        }
        FanOutSelector::Epic(id) => {
            let want = format!("parent:{id}");
            candidates
                .iter()
                .filter(|c| tag_matches(c, &want))
                .collect()
        }
    };

    // For explicit ids, an unknown id is a drop the caller should narrate.
    let mut keep = Vec::new();
    let mut dropped = Vec::new();
    if let FanOutSelector::Specs(ids) = selector {
        for id in *ids {
            if !candidates
                .iter()
                .any(|c| c.spec_id.eq_ignore_ascii_case(id))
            {
                dropped.push(id.clone());
            }
        }
    }

    for c in members {
        if !c.is_approved {
            dropped.push(c.spec_id.clone());
            continue;
        }
        if c.is_low_priority && !include_low {
            dropped.push(c.spec_id.clone());
            continue;
        }
        keep.push(c.spec_id.clone());
    }

    (keep, dropped)
}

/// `aida plan fan-out` — the thin plan-only fan-out driver (STORY-519).
///
/// Resolves a workable set of Approved specs (by explicit list, `--batch`,
/// or `--epic`), then for each spec in turn runs the plan step
/// (`aida queue work <spec> --plan-only`, STORY-265's slice 2) and the
/// `aida plan promote <spec>` Approved -> Planned bump (slice 1). Sequential
/// by design: true parallelism is the harness's job (worktree-isolated
/// agents), and promotion is contention-free pre-work — never a merge — so
/// fan-out can't race the drain (the STORY-519 thesis). `--promote-only`
/// skips the plan-session launch and just runs the lifecycle bumps for specs
/// whose plan file already landed.
// trace:STORY-519 | ai:claude
fn plan_fan_out(
    specs: &[String],
    batch: Option<&str>,
    epic: Option<&str>,
    include_low: bool,
    dry_run: bool,
    promote_only: bool,
) -> Result<()> {
    use colored::Colorize;
    let project_root = find_project_root().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let store = load_store_for_lookup(&project_root)
        .ok_or_else(|| anyhow::anyhow!("could not load the requirement store"))?;

    let selector = match (batch, epic, specs.is_empty()) {
        (Some(b), None, true) => FanOutSelector::Batch(b),
        (None, Some(e), true) => FanOutSelector::Epic(e),
        (None, None, false) => FanOutSelector::Specs(specs),
        (None, None, true) => {
            anyhow::bail!("specify a selection: SPEC-IDs, --batch NAME, or --epic ID")
        }
        _ => anyhow::bail!("--batch, --epic, and an explicit spec list are mutually exclusive"),
    };

    let candidates: Vec<FanOutCandidate> = store
        .requirements
        .iter()
        .filter_map(|r| {
            r.spec_id.clone().map(|sid| FanOutCandidate {
                spec_id: sid,
                is_approved: r.status == aida_core::RequirementStatus::Approved,
                is_low_priority: r.priority == aida_core::RequirementPriority::Low,
                tags: r.tags.iter().cloned().collect(),
            })
        })
        .collect();

    let (set, dropped) = resolve_fan_out_set(&candidates, &selector, include_low);

    let header = match &selector {
        FanOutSelector::Specs(_) => "explicit spec list".to_string(),
        FanOutSelector::Batch(b) => format!("batch:{b}"),
        FanOutSelector::Epic(e) => format!("parent:{e}"),
    };
    println!(
        "{} plan-only fan-out over {} ({} spec{}, {} dropped)",
        "📐".bold(),
        header.cyan(),
        set.len(),
        if set.len() == 1 { "" } else { "s" },
        dropped.len()
    );
    if !dropped.is_empty() {
        let why = if include_low {
            "not Approved"
        } else {
            "not Approved or low-priority tail / unknown"
        };
        println!(
            "  {} dropped ({}): {}",
            "↷".dimmed(),
            why,
            dropped.join(", ")
        );
        // Spell out that the low-priority tail is *intentionally* skipped and
        // how to include it, so dropped specs don't read as silently-missed
        // work. trace:TASK-778
        if !include_low
            && candidates
                .iter()
                .any(|c| c.is_low_priority && c.is_approved && dropped.contains(&c.spec_id))
        {
            println!(
                "  {} low-priority specs are skipped by default; re-run with {} to plan them too.",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).dimmed(),
                "--include-low".cyan()
            );
        }
    }
    if set.is_empty() {
        println!("  nothing to fan out.");
        return Ok(());
    }

    if dry_run {
        for sid in &set {
            let action = if promote_only {
                "promote (plan-only skipped)"
            } else {
                "plan → promote"
            };
            println!(
                "  {} {} → {}",
                crate::glyph(crate::glyphs::Glyph::Bullet).dimmed(),
                sid.bold(),
                action
            );
        }
        println!("(dry-run) {} spec(s) would be processed", set.len());
        return Ok(());
    }

    let exe = std::env::current_exe().context("could not resolve the aida binary path")?;
    let (mut planned, mut promoted, mut failed) = (0usize, 0usize, 0usize);
    for sid in &set {
        if !promote_only {
            println!(
                "\n{} planning {}",
                crate::glyph(crate::glyphs::Glyph::FlowActive).cyan(),
                sid.bold()
            );
            let status = std::process::Command::new(&exe)
                .args(["queue", "work", sid, "--plan-only"])
                .status()
                .with_context(|| format!("could not launch the plan session for {sid}"))?;
            if !status.success() {
                eprintln!(
                    "  {} {} — plan session exited with status {}; skipping promote",
                    crate::glyph(crate::glyphs::Glyph::Cross).red(),
                    sid,
                    status.code().unwrap_or(-1)
                );
                failed += 1;
                continue;
            }
            planned += 1;
        }

        let status = std::process::Command::new(&exe)
            .args(["plan", "promote", sid])
            .status()
            .with_context(|| format!("could not run `aida plan promote {sid}`"))?;
        if status.success() {
            promoted += 1;
        } else {
            eprintln!(
                "  {} {} — promote exited with status {} (did the plan file's `Specs:` header list {sid}?)",
                crate::glyph(crate::glyphs::Glyph::Cross).red(),
                sid,
                status.code().unwrap_or(-1)
            );
            failed += 1;
        }
    }

    println!(
        "\n{} fan-out done: {} planned, {} promoted, {} failed",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        planned,
        promoted,
        failed
    );
    if failed > 0 {
        // Non-zero so a harness can triage partial fan-outs.
        std::process::exit(2);
    }
    Ok(())
}

/// CLI entry point for `aida plan verify <file> [--fix] [--quiet]`. Reads the
/// plan, runs the pure `compute_plan_report` pass, prints the grouped findings,
/// optionally rewrites drifted refs in place, and exits non-zero on errors
/// (pre-commit-hook-able).
// trace:TASK-93 | ai:claude
fn verify_plan(plan_file: &std::path::Path, fix: bool, quiet: bool) -> Result<()> {
    let content = std::fs::read_to_string(plan_file)
        .with_context(|| format!("could not read plan file {}", plan_file.display()))?;
    let root = plan_repo_root(plan_file);
    let lines: Vec<&str> = content.lines().collect();
    let report = compute_plan_report(&content, &root);
    let PlanReport {
        sections: section_findings,
        files: file_findings,
        refs: ref_findings,
        fixes,
    } = &report;

    // --- Report. ---
    let plan_label = plan_file.display();
    println!("{} {}", "Verifying plan:".bold(), plan_label);
    println!();

    let print_group = |title: &str, findings: &[PlanFinding]| {
        if findings.is_empty() {
            return;
        }
        println!("{}", title.cyan().bold());
        for f in findings {
            if quiet && f.level == PlanFindingLevel::Ok {
                continue;
            }
            let tag = match f.level {
                PlanFindingLevel::Ok => "  OK   ".green(),
                PlanFindingLevel::Warn => "  WARN ".yellow(),
                PlanFindingLevel::Error => "  ERROR".red().bold(),
            };
            println!("{} {}", tag, f.msg);
        }
        println!();
    };

    print_group("Sections", section_findings);
    print_group("Files", file_findings);
    print_group("Line refs", ref_findings);

    let errors = report.error_count();
    let warns = report.warn_count();

    if fix && !fixes.is_empty() {
        let mut patched = lines.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut applied = 0;
        for f in fixes.iter() {
            if let Some(line) = patched.get_mut(f.line_idx) {
                if line.contains(&f.old) {
                    *line = line.replacen(&f.old, &f.new, 1);
                    applied += 1;
                }
            }
        }
        let trailing_nl = content.ends_with('\n');
        let mut new_content = patched.join("\n");
        if trailing_nl {
            new_content.push('\n');
        }
        std::fs::write(plan_file, new_content)?;
        println!(
            "{} rewrote {} drifted ref(s) in place",
            "fix:".green().bold(),
            applied
        );
        println!();
    } else if !fixes.is_empty() {
        println!(
            "{} re-run with {} to rewrite {} drifted ref(s) automatically",
            "hint:".bold(),
            "--fix".cyan(),
            fixes.len()
        );
        println!();
    }

    let verdict = if errors > 0 {
        format!("{} error(s), {} warning(s) — FAIL", errors, warns)
            .red()
            .bold()
            .to_string()
    } else if warns > 0 {
        format!("0 errors, {warns} warning(s) — PASS")
            .yellow()
            .to_string()
    } else {
        "all checks passed — PASS".green().bold().to_string()
    };
    println!("{} {}", "Verdict:".bold(), verdict);

    if errors > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// `aida plan helpers <spec>` — render a `## Reusable helpers` section
/// derived from the trace graph.
// trace:TASK-94 | ai:claude
fn plan_helpers(spec_arg: &str, append: Option<&std::path::Path>) -> Result<()> {
    let project_root = find_project_root()?;
    let store = load_store_for_lookup(&project_root)
        .ok_or_else(|| anyhow::anyhow!("could not load the AIDA requirements store"))?;
    let target = if let Ok(uuid) = uuid::Uuid::parse_str(spec_arg) {
        store.requirements.iter().find(|r| r.id == uuid)
    } else {
        store.get_requirement_by_spec_id(spec_arg)
    }
    .ok_or_else(|| anyhow::anyhow!("requirement `{spec_arg}` not found"))?;
    let target_display = target.display_id();

    let Some(mut md) = build_reusable_helpers_section(&store, &project_root, target) else {
        println!(
            "No reusable helpers derived for {target_display} — no related spec (sibling / \
             tag-mate / same-feature) carries a `trace:` comment that names a helper."
        );
        return Ok(());
    };
    md.push_str(&format!(
        "\n_Generated by `aida plan helpers {target_display}` — verify before relying on it._\n"
    ));

    if let Some(path) = append {
        let mut existing = std::fs::read_to_string(path)
            .with_context(|| format!("could not read plan file {}", path.display()))?;
        if !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push('\n');
        existing.push_str(&md);
        std::fs::write(path, existing)
            .with_context(|| format!("could not write {}", path.display()))?;
        println!(
            "{} appended Reusable helpers section to {}",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            path.display()
        );
    } else {
        print!("{md}");
    }
    Ok(())
}

// ============================================================================
// TASK-0418 — `aida plan scan <SPEC>`. An OPT-IN, read-only context-grounding
// pass to run BEFORE generating an AIDA plan or importing a Spec-Kit /
// OpenSpec-style artifact. It walks the trace graph to summarize the files +
// symbols the work will touch (current APIs / architectural constraints) and
// flags likely-stale assumptions — code paths the spec text names that no
// longer exist in the tree. The result is provenance: print it, append it to
// a plan file, or `--attach` it to the spec so the grounding travels with the
// work. Read-only unless `--attach` is passed.
// ============================================================================

/// A code path the spec's prose names (a `path/to/file.rs` or a
/// `path/to/file.rs:symbol` ref) that the scan could not locate in the
/// tree — a candidate stale assumption to verify before implementing.
struct StaleCandidate {
    /// The ref exactly as it appeared in the spec text.
    reference: String,
    /// Why it's flagged: the file is gone, or the file exists but the
    /// named symbol is no longer in it.
    reason: String,
}

/// The assembled pre-plan scan for one spec. Serializable so `--json` can
/// hand it to an external artifact generator.
// trace:TASK-0418
#[derive(serde::Serialize)]
struct PreplanScan {
    spec: String,
    title: String,
    /// Files the trace graph says this area touches, with the symbols
    /// related specs defined there — the current-API surface.
    api_surface: Vec<ScanFile>,
    /// Architectural-constraint notes harvested read-only from the graph
    /// (related-spec relationships, parent epics).
    constraints: Vec<String>,
    /// Likely-stale assumptions: code paths the spec names that are gone.
    stale_assumptions: Vec<ScanStale>,
}

#[derive(serde::Serialize)]
struct ScanFile {
    file: String,
    symbols: Vec<String>,
}

#[derive(serde::Serialize)]
struct ScanStale {
    reference: String,
    reason: String,
}

/// Collect every `path:line`-free file/symbol ref the spec prose names that
/// looks like a real code path, and check each against the tree. Returns the
/// ones that no longer resolve — the stale-assumption candidates. Read-only.
// trace:TASK-0418
fn scan_stale_assumptions(
    project_root: &std::path::Path,
    description: &str,
) -> Vec<StaleCandidate> {
    use regex::Regex;
    // A path-like token: at least one `/` and a known source extension,
    // optionally with a `:symbol` (NOT `:line` — a numeric suffix is a line
    // ref, which drifts and isn't a stale-FILE signal). Captured from inline
    // code spans and bare prose alike.
    let re = Regex::new(
        r"([A-Za-z0-9_./-]+/[A-Za-z0-9_.-]+\.(?:rs|ts|tsx|js|jsx|py|sh|toml|md))(?::([A-Za-z_][A-Za-z0-9_]*))?",
    )
    .unwrap();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<StaleCandidate> = Vec::new();
    for cap in re.captures_iter(description) {
        let file = cap[1].trim_start_matches("./").to_string();
        let symbol = cap.get(2).map(|m| m.as_str().to_string());
        let key = match &symbol {
            Some(s) => format!("{file}:{s}"),
            None => file.clone(),
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        let abs = project_root.join(&file);
        if !abs.exists() {
            out.push(StaleCandidate {
                reference: key,
                reason: format!("file `{file}` not found in the tree"),
            });
            continue;
        }
        // File exists — if a symbol was named, confirm it's still defined.
        if let Some(sym) = &symbol {
            let Ok(content) = std::fs::read_to_string(&abs) else {
                continue;
            };
            let def = Regex::new(&format!(
                r"\b(?:fn|struct|enum|trait|type|mod|const|static|function|class|interface)\s+{}\b",
                regex::escape(sym)
            ))
            .unwrap();
            if !def.is_match(&content) {
                out.push(StaleCandidate {
                    reference: key,
                    reason: format!("symbol `{sym}` no longer defined in `{file}`"),
                });
            }
        }
    }
    out
}

/// Assemble the read-only pre-plan scan for `target`: the trace-graph API
/// surface, architectural-constraint notes, and stale-assumption candidates.
/// Pure (no I/O beyond the read-only source scan) so it's unit-testable.
// trace:TASK-0418
fn build_preplan_scan(
    store: &aida_core::RequirementsStore,
    project_root: &std::path::Path,
    target: &aida_core::models::Requirement,
) -> PreplanScan {
    let display = target.display_id();

    // ── API surface: the files + symbols related specs trace to. Reuse the
    // same related-spec discovery + trace scan the reusable-helpers section
    // is built on, but keep file-only hits too — a file every related spec
    // touches is itself an architectural-constraint signal here.
    let parent_uuids: Vec<uuid::Uuid> = target
        .relationships
        .iter()
        .filter(|r| r.rel_type == aida_core::RelationshipType::Child)
        .map(|r| r.target_id)
        .collect();
    let mut related_ids: HashSet<String> = HashSet::new();
    let mut seen: HashSet<uuid::Uuid> = HashSet::new();
    seen.insert(target.id);
    let push_ids = |req: &aida_core::models::Requirement, set: &mut HashSet<String>| {
        if let Some(s) = &req.spec_id {
            set.insert(s.clone());
        }
        if let Some(a) = &req.agreed_id {
            set.insert(a.clone());
        }
    };
    // The target itself is part of the surface (it may already have code).
    push_ids(target, &mut related_ids);
    if !parent_uuids.is_empty() || !target.tags.is_empty() {
        for req in &store.requirements {
            if seen.contains(&req.id) {
                continue;
            }
            let is_sibling = req.relationships.iter().any(|r| {
                r.rel_type == aida_core::RelationshipType::Child
                    && parent_uuids.contains(&r.target_id)
            });
            let shares_tag = req.tags.iter().any(|t| target.tags.contains(t));
            if is_sibling || shares_tag {
                seen.insert(req.id);
                push_ids(req, &mut related_ids);
            }
        }
    }

    let hits = scan_trace_graph(project_root, &related_ids);
    let mut by_file: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for id_hits in hits.values() {
        for hit in id_hits {
            let syms = by_file.entry(hit.file.clone()).or_default();
            if let Some(sym) = &hit.symbol {
                if !syms.contains(sym) {
                    syms.push(sym.clone());
                }
            }
        }
    }
    const SCAN_FILE_CAP: usize = 25;
    const SCAN_SYM_PER_FILE_CAP: usize = 12;
    let mut api_surface: Vec<ScanFile> = by_file
        .into_iter()
        .map(|(file, mut symbols)| {
            symbols.sort();
            symbols.truncate(SCAN_SYM_PER_FILE_CAP);
            ScanFile { file, symbols }
        })
        .collect();
    // Most-symbol-dense files first — the densest API surface leads.
    api_surface.sort_by(|a, b| {
        b.symbols
            .len()
            .cmp(&a.symbols.len())
            .then(a.file.cmp(&b.file))
    });
    api_surface.truncate(SCAN_FILE_CAP);

    // ── Architectural constraints from the graph: parents (the epic/story
    // this slots under) and any non-parent/child typed relationships
    // (blocks / depends-on / verifies) the work must respect.
    let resolve = |id: &uuid::Uuid| store.requirements.iter().find(|r| &r.id == id);
    let mut constraints: Vec<String> = Vec::new();
    for rel in &target.relationships {
        match &rel.rel_type {
            aida_core::RelationshipType::Child => {
                if let Some(p) = resolve(&rel.target_id) {
                    constraints.push(format!(
                        "Parent {} — {} ({})",
                        p.display_id(),
                        p.title,
                        p.status
                    ));
                }
            }
            aida_core::RelationshipType::Parent => {}
            other => {
                if let Some(r) = resolve(&rel.target_id) {
                    constraints.push(format!("{}: {} — {}", other, r.display_id(), r.title));
                }
            }
        }
    }

    // ── Stale assumptions: code paths the spec names that are gone.
    let stale_assumptions: Vec<ScanStale> =
        scan_stale_assumptions(project_root, &target.description)
            .into_iter()
            .map(|c| ScanStale {
                reference: c.reference,
                reason: c.reason,
            })
            .collect();

    PreplanScan {
        spec: display,
        title: target.title.clone(),
        api_surface,
        constraints,
        stale_assumptions,
    }
}

/// Render a [`PreplanScan`] as the `## Pre-plan scan` markdown section —
/// the provenance shape that prints, appends to a plan file, or attaches to
/// the spec.
// trace:TASK-0418
fn render_preplan_scan(scan: &PreplanScan) -> String {
    let mut md = String::new();
    md.push_str("## Pre-plan scan\n\n");
    md.push_str(&format!(
        "Read-only context-grounding scan for {} — {}. Verify before relying on it; \
         the tree moves.\n\n",
        scan.spec, scan.title
    ));

    md.push_str("### Current API surface (trace-graph derived)\n\n");
    if scan.api_surface.is_empty() {
        md.push_str(
            "_No related spec traces to code yet — this is greenfield for the scanner. \
             Ground the plan by reading the modules it will touch directly._\n\n",
        );
    } else {
        for sf in &scan.api_surface {
            if sf.symbols.is_empty() {
                md.push_str(&format!("- `{}`\n", sf.file));
            } else {
                let joined = sf
                    .symbols
                    .iter()
                    .map(|s| format!("`{s}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                md.push_str(&format!("- `{}` — {}\n", sf.file, joined));
            }
        }
        md.push('\n');
    }

    md.push_str("### Architectural constraints\n\n");
    if scan.constraints.is_empty() {
        md.push_str("_No graph-level constraints (no parent / typed relationships)._\n\n");
    } else {
        for c in &scan.constraints {
            md.push_str(&format!("- {c}\n"));
        }
        md.push('\n');
    }

    md.push_str("### Likely-stale assumptions\n\n");
    if scan.stale_assumptions.is_empty() {
        md.push_str(
            "_No code path named in the spec is missing from the tree — assumptions look current._\n\n",
        );
    } else {
        for s in &scan.stale_assumptions {
            md.push_str(&format!("- `{}` — {}\n", s.reference, s.reason));
        }
        md.push('\n');
    }

    md.push_str(&format!(
        "_Generated by `aida plan scan {}`. Compose with Spec-Kit / OpenSpec generators: \
         run the scan first, feed this summary in as grounding, then re-attach the provenance._\n",
        scan.spec
    ));
    md
}

/// `aida plan scan <SPEC>` — the read-only pre-plan context-grounding pass.
// trace:TASK-0418
fn plan_scan(
    spec_arg: &str,
    attach: bool,
    append: Option<&std::path::Path>,
    json: bool,
) -> Result<()> {
    let project_root = find_project_root()?;
    let store = load_store_for_lookup(&project_root)
        .ok_or_else(|| anyhow::anyhow!("could not load the AIDA requirements store"))?;
    let target = if let Ok(uuid) = uuid::Uuid::parse_str(spec_arg) {
        store.requirements.iter().find(|r| r.id == uuid)
    } else {
        store
            .requirements
            .iter()
            .find(|r| r.matches_id(spec_arg))
            .or_else(|| store.get_requirement_by_spec_id(spec_arg))
    }
    .ok_or_else(|| anyhow::anyhow!("requirement `{spec_arg}` not found"))?;
    let target_display = target.display_id();

    let scan = build_preplan_scan(&store, &project_root, target);

    if json {
        println!("{}", serde_json::to_string_pretty(&scan)?);
        // --json is the machine surface; skip the human prints, but still
        // honor the explicit write flags below.
    } else {
        print!("{}", render_preplan_scan(&scan));
    }

    let md = render_preplan_scan(&scan);

    if let Some(path) = append {
        let mut existing = std::fs::read_to_string(path)
            .with_context(|| format!("could not read plan file {}", path.display()))?;
        if !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push('\n');
        existing.push_str(&md);
        std::fs::write(path, existing)
            .with_context(|| format!("could not write {}", path.display()))?;
        if !json {
            println!(
                "{} appended Pre-plan scan section to {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                path.display()
            );
        }
    }

    if attach {
        // The only write the command performs. Open a writable backend the
        // same way the findings-add path does, and land the scan as a
        // provenance comment on the spec.
        let Some(store_path) = detect_distributed_store_from(&project_root) else {
            anyhow::bail!(
                "--attach needs a distributed store, but none was found — \
                 run from a project with `.aida/config.toml`."
            );
        };
        let dispenser = load_dispenser(&store_path)?;
        let inner = aida_core::GitBackend::new(&store_path)?.with_dispenser(dispenser);
        let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
        let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;
        let mut req = backend
            .get_requirement_by_spec_id(&target_display)?
            .or(backend.get_requirement_by_spec_id(spec_arg)?)
            .ok_or_else(|| {
                anyhow::anyhow!("requirement `{target_display}` not found for attach")
            })?;
        let now = chrono::Utc::now();
        req.comments.push(Comment {
            id: Uuid::now_v7(),
            content: format!("Pre-plan context-grounding scan:\n\n{md}"),
            author: get_default_author(),
            created_at: now,
            modified_at: now,
            parent_id: None,
            replies: Vec::new(),
            reactions: Vec::new(),
            session_id: resolve_current_session_id(), // trace:TASK-330
        });
        req.modified_at = now;
        backend.update_requirement(&req)?;
        if !json {
            println!(
                "{} attached pre-plan scan as a provenance comment on {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                target_display
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    /// TASK-0418: the pre-plan scan's stale-assumption detector flags code
    /// paths the spec names that no longer resolve — a missing file and a
    /// missing symbol — while leaving present file+symbol refs alone and
    /// ignoring `path:line` refs (a numeric suffix is a drift-prone line ref,
    /// not a stale-FILE signal).
    #[test]
    fn preplan_scan_flags_stale_assumptions() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("aida-cli/src");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("real.rs"), "pub fn still_here() {}\n").unwrap();

        let desc = "Touches `aida-cli/src/real.rs:still_here` and \
                    `aida-cli/src/real.rs:gone_now`. Also see \
                    `aida-cli/src/deleted.rs`. Line ref aida-cli/src/real.rs:42 \
                    should NOT be flagged.";
        let stale = scan_stale_assumptions(dir.path(), desc);
        let refs: Vec<&str> = stale.iter().map(|s| s.reference.as_str()).collect();

        // Missing file and missing symbol are flagged.
        assert!(refs.contains(&"aida-cli/src/deleted.rs"));
        assert!(refs.contains(&"aida-cli/src/real.rs:gone_now"));
        // The present file+symbol is NOT flagged.
        assert!(!refs.contains(&"aida-cli/src/real.rs:still_here"));
        // The `:42` line ref collapses to the bare (present) file and is
        // therefore not flagged as missing.
        assert!(!refs.iter().any(|r| r.contains(":42")));
    }

    /// TASK-0418: the markdown renderer always emits the three grounding
    /// sections and the Spec-Kit / OpenSpec composition footer, even when a
    /// section is empty (greenfield / no constraints / no stale refs).
    #[test]
    fn preplan_scan_renders_all_sections() {
        let scan = PreplanScan {
            spec: "TASK-9".to_string(),
            title: "Do the thing".to_string(),
            api_surface: vec![ScanFile {
                file: "src/lib.rs".to_string(),
                symbols: vec!["do_thing".to_string()],
            }],
            constraints: vec![],
            stale_assumptions: vec![ScanStale {
                reference: "src/old.rs".to_string(),
                reason: "file `src/old.rs` not found in the tree".to_string(),
            }],
        };
        let md = render_preplan_scan(&scan);
        assert!(md.contains("## Pre-plan scan"));
        assert!(md.contains("### Current API surface"));
        assert!(md.contains("`src/lib.rs` — `do_thing`"));
        assert!(md.contains("### Architectural constraints"));
        assert!(md.contains("No graph-level constraints"));
        assert!(md.contains("### Likely-stale assumptions"));
        assert!(md.contains("`src/old.rs`"));
        assert!(md.contains("Spec-Kit / OpenSpec"));
    }

    /// TASK-305: the pure PR→plan synthesis fills all 11 sections from a PR
    /// fixture, mines verification commands from the description, lists the
    /// changed files under Critical Files, and credits the spec ids found in
    /// the commit log. Then `plan_sections_present` (the same check
    /// `aida plan verify` runs) accepts the output.
    #[test]
    fn plan_capture_synthesizes_verifiable_plan() {
        let pr = CapturedPr {
            number: 65,
            title: "feat(plan): web flow plumbing (STORY-278)".to_string(),
            body:
                "## Summary\n\nWire the web /ultraplan flow end to end.\n\n## Testing\n\n```bash\n\
                   cargo test -p aida-cli --release\naida plan verify docs/plans/x.md\n```\n"
                    .to_string(),
            commit_subjects: vec![
                "[AI:claude] feat(plan): step one (STORY-278)".to_string(),
                "[AI:claude] feat(plan): step two (STORY-278)".to_string(),
            ],
            changed_files: vec![
                "aida-cli/src/main.rs".to_string(),
                "aida-cli/src/cli.rs".to_string(),
            ],
        };

        let md = synthesize_plan_from_pr(&pr, "2026-06-06");

        // Header carries Date / Specs / Status / Complexity / Source.
        assert!(md.contains("Date: 2026-06-06"));
        assert!(md.contains("Specs: STORY-278"));
        assert!(md.contains("Source: web /ultraplan PR-65"));
        assert!(md.contains("Status: Completed"));
        assert!(md.contains("Complexity: 2 commits"));

        // Approach pulls the PR body; Critical Files lists the diff.
        assert!(md.contains("Wire the web /ultraplan flow end to end."));
        assert!(md.contains("- `aida-cli/src/main.rs`"));
        assert!(md.contains("- `aida-cli/src/cli.rs`"));

        // Verification mines the command-shaped lines from the body.
        assert!(md.contains("cargo test -p aida-cli --release"));
        assert!(md.contains("aida plan verify docs/plans/x.md"));

        // Commit log becomes the build-order steps.
        assert!(md.contains("1. [AI:claude] feat(plan): step one (STORY-278)"));

        // The same section check `aida plan verify` runs must pass — every
        // hard-required section present.
        for spec in PLAN_SECTIONS {
            let present = md.lines().any(|l| {
                let lower = l.to_ascii_lowercase();
                l.starts_with("##")
                    && lower.contains(spec.keyword)
                    && spec.exclude.map(|ex| !lower.contains(ex)).unwrap_or(true)
            });
            assert!(present, "captured plan missing section: {}", spec.label);
        }

        // Idempotent / deterministic: same input → byte-identical output.
        let md2 = synthesize_plan_from_pr(&pr, "2026-06-06");
        assert_eq!(md, md2);

        // Slug threads the spec id + PR number for a stable filename.
        assert_eq!(captured_plan_slug(&pr), "story-278-from-pr-65");
    }

    /// TASK-305: a PR with no body / no commits / no diff still yields a
    /// fully-sectioned plan with explicit not-captured markers (so
    /// `aida plan verify` still passes) and a PR-number-based slug.
    #[test]
    fn plan_capture_handles_empty_pr() {
        let pr = CapturedPr {
            number: 7,
            title: String::new(),
            body: String::new(),
            commit_subjects: vec![],
            changed_files: vec![],
        };
        let md = synthesize_plan_from_pr(&pr, "2026-06-06");
        assert!(md.contains("# Plan: PR-7"));
        assert!(md.contains("Specs: <!-- not captured from PR -->"));
        assert!(md.contains("<!-- not captured from PR -->"));
        // All hard-required sections still present.
        for spec in PLAN_SECTIONS.iter().filter(|s| s.hard) {
            assert!(
                md.lines()
                    .any(|l| l.starts_with("##") && l.to_ascii_lowercase().contains(spec.keyword)),
                "missing hard section: {}",
                spec.label
            );
        }
        assert_eq!(captured_plan_slug(&pr), "pr-7");
    }

    /// BUG-473: `extract_verification_commands` keeps a looser set of commands
    /// inside a fenced block (any non-empty, non-comment line) than outside (only
    /// tool-prefixed lines). The old `(in_fence && looks_like_cmd) || looks_like_cmd`
    /// collapsed to `looks_like_cmd`, so fenced non-tool commands were dropped.
    #[test]
    fn extract_verification_commands_keeps_fenced_non_tool_lines() {
        let body = "\
prose before the block should never be captured\n\
RUSTFLAGS=-Awarnings should not leak outside a fence\n\
```bash\n\
# this is a comment, skip it\n\
RUSTFLAGS=-Awarnings cargo build\n\
cd subdir && make check\n\
cargo test -p aida-cli\n\
```\n\
trailing prose after the block, also skipped\n";
        let cmds = extract_verification_commands(body);
        // Fenced non-tool-prefixed commands are now captured (the BUG-473 fix).
        assert!(
            cmds.contains(&"RUSTFLAGS=-Awarnings cargo build".to_string()),
            "fenced env-prefixed command should be captured: {cmds:?}"
        );
        assert!(
            cmds.contains(&"cd subdir && make check".to_string()),
            "fenced `cd && make` command should be captured: {cmds:?}"
        );
        // Tool-prefixed fenced command still captured.
        assert!(cmds.contains(&"cargo test -p aida-cli".to_string()));
        // Comment lines inside the fence are skipped.
        assert!(!cmds.iter().any(|c| c.starts_with('#')));
        // Prose / env line OUTSIDE the fence is still rejected (tool-prefix gate).
        assert!(!cmds.iter().any(|c| c.contains("should not leak outside")));
        assert!(!cmds.iter().any(|c| c.contains("prose before")));
        assert!(!cmds.iter().any(|c| c.contains("trailing prose")));
    }

    /// TASK-305: PR arg parsing accepts bare / PR- / # forms.
    #[test]
    fn plan_capture_parses_pr_arg() {
        assert_eq!(parse_pr_number("65").unwrap(), 65);
        assert_eq!(parse_pr_number("PR-65").unwrap(), 65);
        assert_eq!(parse_pr_number("pr-65").unwrap(), 65);
        assert_eq!(parse_pr_number("#65").unwrap(), 65);
        assert!(parse_pr_number("not-a-pr").is_err());
    }

    /// STORY-519: helper to build a fan-out candidate for the selector tests.
    fn fc(spec_id: &str, approved: bool, low: bool, tags: &[&str]) -> FanOutCandidate {
        FanOutCandidate {
            spec_id: spec_id.to_string(),
            is_approved: approved,
            is_low_priority: low,
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    /// STORY-519: a `--batch NAME` selection keeps Approved batch members,
    /// drops the low-priority tail by default, and drops non-Approved members.
    #[test]
    fn fan_out_batch_excludes_low_tail_and_non_approved() {
        let candidates = vec![
            fc("STORY-1", true, false, &["batch:burndown"]),
            fc("TASK-2", true, true, &["batch:burndown"]), // low-priority tail
            fc("TASK-3", false, false, &["batch:burndown"]), // not Approved
            fc("TASK-4", true, false, &["batch:other"]),   // different batch
        ];
        let (set, dropped) =
            resolve_fan_out_set(&candidates, &FanOutSelector::Batch("burndown"), false);
        assert_eq!(set, vec!["STORY-1"]);
        assert!(dropped.contains(&"TASK-2".to_string()));
        assert!(dropped.contains(&"TASK-3".to_string()));
        // A spec in a different batch is never a member → not even "dropped".
        assert!(!dropped.contains(&"TASK-4".to_string()));
    }

    /// STORY-519: `--include-low` keeps the low-priority members but still
    /// drops non-Approved ones.
    #[test]
    fn fan_out_include_low_keeps_low_priority() {
        let candidates = vec![
            fc("STORY-1", true, false, &["batch:burndown"]),
            fc("TASK-2", true, true, &["batch:burndown"]),
            fc("TASK-3", false, true, &["batch:burndown"]),
        ];
        let (set, dropped) =
            resolve_fan_out_set(&candidates, &FanOutSelector::Batch("burndown"), true);
        assert_eq!(set, vec!["STORY-1", "TASK-2"]);
        // Non-Approved still dropped even with include_low.
        assert_eq!(dropped, vec!["TASK-3"]);
    }

    /// STORY-519: `--epic ID` selects by the `parent:ID` rollup tag.
    #[test]
    fn fan_out_epic_selects_by_parent_tag() {
        let candidates = vec![
            fc("STORY-1", true, false, &["parent:EPIC-7"]),
            fc("STORY-2", true, false, &["parent:EPIC-7", "batch:x"]),
            fc("STORY-3", true, false, &["parent:EPIC-9"]),
        ];
        let (set, dropped) =
            resolve_fan_out_set(&candidates, &FanOutSelector::Epic("EPIC-7"), false);
        assert_eq!(set, vec!["STORY-1", "STORY-2"]);
        assert!(dropped.is_empty());
    }

    /// STORY-519: an explicit spec list keeps the requested order, drops a
    /// non-Approved id, and reports an unknown id as dropped.
    #[test]
    fn fan_out_explicit_list_order_and_unknown() {
        let candidates = vec![
            fc("STORY-1", true, false, &[]),
            fc("TASK-2", false, false, &[]),
        ];
        let ids = vec![
            "TASK-2".to_string(),
            "STORY-1".to_string(),
            "GHOST-9".to_string(),
        ];
        let (set, dropped) = resolve_fan_out_set(&candidates, &FanOutSelector::Specs(&ids), false);
        // STORY-1 kept; order follows the request (only one survives here).
        assert_eq!(set, vec!["STORY-1"]);
        // Non-Approved TASK-2 and unknown GHOST-9 both dropped.
        assert!(dropped.contains(&"TASK-2".to_string()));
        assert!(dropped.contains(&"GHOST-9".to_string()));
    }

    /// STORY-519: explicit-id matching is case-insensitive and preserves the
    /// store's canonical id in the kept set.
    #[test]
    fn fan_out_explicit_case_insensitive() {
        let candidates = vec![fc("STORY-42", true, false, &[])];
        let ids = vec!["story-42".to_string()];
        let (set, dropped) = resolve_fan_out_set(&candidates, &FanOutSelector::Specs(&ids), false);
        assert_eq!(set, vec!["STORY-42"]);
        assert!(dropped.is_empty());
    }

    #[test]
    fn plan_specs_parses_header_line() {
        // STORY-265: the Specs: header, comma + space separated.
        let content = "# Plan: thing\n\nDate: 2026-06-06\nSpecs: STORY-265, BUG-12 TASK-1-097\nStatus: Draft\n\n## Body\nSpecs: NOT-THIS-ONE\n";
        assert_eq!(
            parse_plan_specs(content),
            vec!["STORY-265", "BUG-12", "TASK-1-097"]
        );
        assert!(parse_plan_specs("# no specs line here\n").is_empty());
    }

    #[test]
    fn find_plan_file_matches_spec_and_skips_template() {
        // STORY-265: finds the plan whose Specs: lists the spec (case-insensitive),
        // skips _TEMPLATE.md, ignores non-matching plans, newest date-prefix wins.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(
            dir.join("_TEMPLATE.md"),
            "Specs: STORY-265\n", // template must be ignored even if it mentions the spec
        )
        .unwrap();
        std::fs::write(dir.join("2026-06-01-other.md"), "Specs: BUG-99\n").unwrap();
        std::fs::write(
            dir.join("2026-06-05-early.md"),
            "# Plan\nSpecs: story-265\n", // case-insensitive match
        )
        .unwrap();
        std::fs::write(
            dir.join("2026-06-06-late.md"),
            "# Plan\nSpecs: STORY-265, TASK-2\n",
        )
        .unwrap();

        let found = find_plan_file_for_spec(dir, "STORY-265").unwrap();
        assert_eq!(found.file_name().unwrap(), "2026-06-06-late.md"); // newest wins
        assert!(find_plan_file_for_spec(dir, "STORY-999").is_none());
        // a spec only present in the template is NOT matched
        assert!(find_plan_file_for_spec(dir, "BUG-99").is_some()); // (real plan)
    }
}
