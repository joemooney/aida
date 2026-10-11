//! `aida config` snapshot, status/priority input validation and init-time config writers (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

pub fn validate_status_input(raw: &str) -> Result<&'static str, String> {
    let normalized: String = raw
        .chars()
        .filter_map(|c| match c {
            ' ' | '-' | '_' => None,
            c if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
            c => Some(c),
        })
        .collect();
    match normalized.as_str() {
        "draft" => Ok("Draft"),
        "approved" => Ok("Approved"),
        "planned" => Ok("Planned"),
        "inprogress" => Ok("InProgress"),
        // trace:STORY-86 | ai:claude — "done" is now its own state, not an alias for Completed.
        "done" => Ok("Done"),
        "completed" => Ok("Completed"),
        "rejected" => Ok("Rejected"),
        // trace:TASK-1176 | ai:claude — adopted-then-replaced (NOT declined).
        "superseded" => Ok("Superseded"),
        // trace:STORY-332 | ai:claude — the punt/pause state.
        "needsattention" => Ok("NeedsAttention"),
        _ => Err(format!(
            "invalid status `{}` — expected one of: {}",
            raw, VALID_STATUS_INPUTS
        )),
    }
}

/// Same shape, for priority. trace:BUG-47
pub fn validate_priority_input(raw: &str) -> Result<&'static str, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "high" => Ok("High"),
        "medium" | "med" => Ok("Medium"),
        "low" => Ok("Low"),
        _ => Err(format!(
            "invalid priority `{}` — expected one of: high, medium, low",
            raw
        )),
    }
}

/// Detect signs that a `--title` was mangled by shell command-substitution
/// (backticks the user forgot to escape, an unmatched quote that lost the
/// rest of the string). Returns Some(message) if suspicious. Caller should
/// print as a warning — never reject — since false positives are possible.
/// trace:BUG-22 | ai:claude
pub(crate) fn suspicious_title_signal(title: &str) -> Option<String> {
    if title.contains('`') {
        return Some(
            "title contains a backtick — if you meant a literal `, escape it (\\\\`) \
             or quote the whole title in single quotes; otherwise the shell may have \
             mangled it"
                .to_string(),
        );
    }
    // An odd count of unescaped double-quotes is a strong signal of broken quoting.
    let dq_count = title.chars().filter(|c| *c == '"').count();
    if dq_count % 2 == 1 {
        return Some(
            "title contains an unbalanced double-quote — likely a shell-quoting artifact"
                .to_string(),
        );
    }
    None
}

pub(crate) fn config_path_for_project(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root.join(".aida").join("config.toml")
}

pub(crate) fn config_parse_error_message(
    path: &std::path::Path,
    body: &str,
    err: &toml::de::Error,
) -> String {
    let loc = err
        .span()
        .map(|span| byte_offset_line_col(body, span.start))
        .map(|(line, col)| format!(":{line}:{col}"))
        .unwrap_or_default();
    format!(
        "{}{}: failed to parse AIDA config: {err}\n  Fix the TOML syntax, then re-run the command. If this came from `aida pull`, inspect any sibling `{}.conflicted` file and merge it manually.",
        path.display(),
        loc,
        path.display()
    )
}

pub(crate) fn byte_offset_line_col(body: &str, offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut col = 1usize;
    for (idx, ch) in body.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

pub(crate) fn config_conflict_marker_line(body: &str) -> Option<usize> {
    body.lines().enumerate().find_map(|(idx, line)| {
        let trimmed = line.trim_start();
        if trimmed.starts_with("<<<<<<< ")
            || trimmed == "<<<<<<<"
            || trimmed.starts_with("=======")
            || trimmed.starts_with(">>>>>>> ")
            || trimmed == ">>>>>>>"
        {
            Some(idx + 1)
        } else {
            None
        }
    })
}

#[derive(Debug)]
pub(crate) struct ConfigSnapshot {
    pub(crate) path: std::path::PathBuf,
    pub(crate) before: Option<String>,
}

pub(crate) fn known_project_config_paths(
    project_root: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    let aida = project_root.join(".aida");
    vec![aida.join("config.toml"), aida.join("agents.toml")]
}

pub(crate) fn snapshot_known_project_configs(
    project_root: &std::path::Path,
) -> Vec<ConfigSnapshot> {
    known_project_config_paths(project_root)
        .into_iter()
        .map(|path| ConfigSnapshot {
            before: std::fs::read_to_string(&path).ok(),
            path,
        })
        .collect()
}

pub(crate) fn validate_and_restore_project_configs_after_pull(
    snapshots: &[ConfigSnapshot],
) -> Result<()> {
    for snapshot in snapshots {
        let Ok(after) = std::fs::read_to_string(&snapshot.path) else {
            continue;
        };
        let failure = if let Some(line) = config_conflict_marker_line(&after) {
            Some(format!(
                "{}:{line}: conflict marker found in AIDA config after pull",
                snapshot.path.display()
            ))
        } else {
            match toml::from_str::<toml::Value>(&after) {
                Ok(_) => None,
                Err(err) => Some(config_parse_error_message(&snapshot.path, &after, &err)),
            }
        };
        let Some(message) = failure else {
            continue;
        };

        let conflicted = snapshot.path.with_extension("toml.conflicted");
        aida_core::write_atomic(&conflicted, after)
            .with_context(|| format!("failed to quarantine {}", conflicted.display()))?;
        match &snapshot.before {
            Some(before) => {
                // trace:BUG-1025 | ai:codex
                aida_core::write_atomic(&snapshot.path, before.clone())
                    .with_context(|| format!("failed to restore {}", snapshot.path.display()))?;
            }
            None => match std::fs::remove_file(&snapshot.path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("failed to remove {}", snapshot.path.display()));
                }
            },
        }
        anyhow::bail!(
            "{message}\n  Quarantined the conflicted version at {}.\n  Restored the pre-pull version at {}.\n  Manual merge step: compare both files, edit {}, then re-run `aida pull`.",
            conflicted.display(),
            snapshot.path.display(),
            snapshot.path.display()
        );
    }
    Ok(())
}

/// TASK-304: `[ultraplan] mode` governs whether AIDA proactively suggests
/// `aida ultraplan <SPEC>` for chunky specs. Planning is human/agent driven,
/// so the realistic surface is `never | on-demand | suggested` — never a
/// "frequently auto-pull" mode.
/// trace:TASK-304 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UltraplanMode {
    /// `aida ultraplan` refuses with a configured-off message; no clipboard.
    Never,
    /// Default — preserves current behavior: the user runs `aida ultraplan
    /// SPEC` explicitly, no pickup-time hints.
    OnDemand,
    /// Pickup surfaces (`/aida-pickup`, `aida queue work` no-arg head,
    /// `aida queue list` head row) hint when the head spec is chunky.
    Suggested,
}

impl UltraplanMode {
    pub(crate) fn from_token(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "never" => Some(Self::Never),
            "on-demand" | "on_demand" | "ondemand" => Some(Self::OnDemand),
            "suggested" => Some(Self::Suggested),
            _ => None,
        }
    }
}

/// TASK-697: which heuristic the `suggested` mode uses to decide a spec is
/// worth a planning prompt.
///
/// SPIKE-8 (`docs/spikes/2026-06-07-spike-8-ultraplan-comparison.md`) found
/// that `aida ultraplan`'s value is a *context-assembly* aid: it helps most on
/// thin / under-specified specs and least on well-formed ones (which already
/// carry their own `## Proposed shape` + Acceptance — the spec *is* the plan).
/// So acceptance-bullet count *anti-correlates* with where planning helps — a
/// 9-bullet spec with a design block needs planning less than a 2-bullet spec
/// with none. `Thinness` keys on that signal and is the default; the legacy
/// `acceptance-bullets>N` heuristic is still honored for backward compat.
/// trace:TASK-697 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SuggestThreshold {
    /// Legacy (TASK-304): suggest when the `## Acceptance` checkbox count is
    /// strictly greater than `N`.
    BulletCount(usize),
    /// TASK-697: suggest when the spec is *under-specified* — no design
    /// section yet a substantive body (see `is_spec_thin`).
    Thinness,
}

/// TASK-304/TASK-697: parsed `[ultraplan]` config. Both fields default safely
/// so a project with no `[ultraplan]` block (or an unparseable one) keeps the
/// opt-in behavior: `mode = on-demand`, threshold `spec-thinness`.
/// trace:TASK-697 | ai:claude
pub(crate) struct UltraplanConfig {
    pub(crate) mode: UltraplanMode,
    /// Which heuristic `suggested` mode applies. Defaults to `Thinness`.
    pub(crate) threshold: SuggestThreshold,
}

impl Default for UltraplanConfig {
    fn default() -> Self {
        Self {
            mode: UltraplanMode::OnDemand,
            threshold: SuggestThreshold::Thinness,
        }
    }
}

/// TASK-304: read `[ultraplan]` from `.aida/config.toml`. Missing file,
/// missing block, unparseable TOML, or unknown token values all fall back to
/// the defaults — the suggestion layer is a soft feature, never load-bearing.
/// trace:TASK-304 | ai:claude
pub(crate) fn read_ultraplan_config(project_root: &std::path::Path) -> UltraplanConfig {
    let mut cfg = UltraplanConfig::default();
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return cfg;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return cfg;
    };
    let Some(table) = value.get("ultraplan") else {
        return cfg;
    };
    if let Some(mode) = table
        .get("mode")
        .and_then(|v| v.as_str())
        .and_then(UltraplanMode::from_token)
    {
        cfg.mode = mode;
    }
    if let Some(threshold) = table
        .get("suggest_threshold")
        .and_then(|v| v.as_str())
        .and_then(parse_suggest_threshold)
    {
        cfg.threshold = threshold;
    }
    cfg
}

/// TASK-304: parse a `suggest_threshold` token. Only `acceptance-bullets>N`
/// is honored today (the spec's chosen default heuristic — mechanical, no
/// NLP, falsifiable); the `N` is extracted. Unknown tokens return `None` so
/// the caller keeps the default threshold. trace:TASK-304 | ai:claude
pub(crate) fn parse_acceptance_bullet_threshold(token: &str) -> Option<usize> {
    token
        .trim()
        .to_ascii_lowercase()
        .strip_prefix("acceptance-bullets>")?
        .trim()
        .parse::<usize>()
        .ok()
}

/// TASK-697: parse a `suggest_threshold` token into a `SuggestThreshold`.
/// `acceptance-bullets>N` → `BulletCount(N)` (legacy, still honored);
/// `spec-thinness` / `thinness` / `under-specified` → `Thinness` (the SPIKE-8
/// recommendation, the new default). Unknown tokens return `None` so the
/// caller keeps the default threshold. trace:TASK-697 | ai:claude
pub(crate) fn parse_suggest_threshold(token: &str) -> Option<SuggestThreshold> {
    if let Some(n) = parse_acceptance_bullet_threshold(token) {
        return Some(SuggestThreshold::BulletCount(n));
    }
    match token.trim().to_ascii_lowercase().as_str() {
        "spec-thinness" | "thinness" | "under-specified" => Some(SuggestThreshold::Thinness),
        _ => None,
    }
}

/// TASK-697: heading prefixes that mark a spec as carrying its own design —
/// the signal that planning would add little (the spec already *is* a plan).
/// Matched case-insensitively against the heading text after the `#` markers.
/// trace:TASK-697 | ai:claude
pub(crate) const DESIGN_SECTION_MARKERS: &[&str] = &[
    "proposed shape",
    "proposed solution",
    "proposed design",
    "design",
    "approach",
    "implementation",
];

/// TASK-697: a spec with `< THIN_MIN_BODY_CHARS` of body is too trivial to
/// plan — planning overhead would exceed the work. Above it, a design-less
/// spec is "thin" in the sense that matters (under-specified, not too-small).
pub(crate) const THIN_MIN_BODY_CHARS: usize = 240;

/// TASK-697: does the spec carry a design / proposed-shape section? Scans
/// markdown headings (any level) for a `DESIGN_SECTION_MARKERS` prefix.
/// trace:TASK-697 | ai:claude
pub(crate) fn has_design_section(description: &str) -> bool {
    for line in description.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let heading = rest.trim_start_matches('#').trim().to_ascii_lowercase();
            if DESIGN_SECTION_MARKERS
                .iter()
                .any(|marker| heading.starts_with(marker))
            {
                return true;
            }
        }
    }
    false
}

/// TASK-697: is this spec "thin" — under-specified enough that a planning
/// prompt would plausibly add value? True when it has NO design section yet a
/// substantive body. Well-specified specs (which embed `## Proposed shape` and
/// the like) stay quiet — exactly the SPIKE-8 finding that bullet count is the
/// wrong signal. Trivial one-liners also stay quiet (too small to plan).
/// trace:TASK-697 | ai:claude
pub(crate) fn is_spec_thin(description: &str) -> bool {
    !has_design_section(description) && description.trim().chars().count() >= THIN_MIN_BODY_CHARS
}

/// TASK-304: count the markdown task-list bullets (`- [ ]` / `- [x]`) inside
/// a spec's `## Acceptance` section — a rough complexity proxy. Returns 0
/// when there's no Acceptance section. Checked and unchecked items both
/// count: a long checklist is chunky regardless of how much is already
/// ticked. Tolerates `*`/`+` bullet markers and `## Acceptance Criteria`
/// heading variants. trace:TASK-304 | ai:claude
pub(crate) fn count_acceptance_bullets(description: &str) -> usize {
    let mut in_section = false;
    let mut count = 0;
    for line in description.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('#') {
            // A markdown heading at any level — entering or leaving the
            // Acceptance section resets the counter scope.
            let heading = rest.trim_start_matches('#').trim().to_ascii_lowercase();
            in_section = heading.starts_with("acceptance");
            continue;
        }
        if in_section && is_task_bullet(trimmed) {
            count += 1;
        }
    }
    count
}

/// TASK-304: is this a markdown task-list item (`- [ ]`, `* [x]`, `+ [X]`)?
pub(crate) fn is_task_bullet(trimmed: &str) -> bool {
    let after_marker = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "));
    match after_marker {
        Some(rest) => {
            let r = rest.trim_start();
            r.starts_with("[ ]") || r.starts_with("[x]") || r.starts_with("[X]")
        }
        None => false,
    }
}

/// TASK-304: the pickup-time suggestion hint. Returns `Some(message)` only
/// when `[ultraplan] mode = "suggested"` AND the spec's acceptance checklist
/// is longer than the configured threshold; `None` otherwise (the common
/// case — on-demand/never, or a spec that isn't chunky). The message format
/// is fixed by the spec so downstream surfaces render it identically.
/// trace:TASK-304 | ai:claude
pub(crate) fn ultraplan_suggestion_hint(
    project_root: &std::path::Path,
    req: &aida_core::Requirement,
) -> Option<String> {
    let cfg = read_ultraplan_config(project_root);
    if cfg.mode != UltraplanMode::Suggested {
        return None;
    }
    let id = req.display_id();
    match cfg.threshold {
        // Legacy (TASK-304): fire on a long acceptance checklist.
        SuggestThreshold::BulletCount(n) => {
            let bullets = count_acceptance_bullets(&req.description);
            if bullets <= n {
                return None;
            }
            Some(format!(
                "{id} has {bullets} acceptance bullets — `aida ultraplan {id}` to \
                 assemble a planning prompt before implementing."
            ))
        }
        // TASK-697: fire on an under-specified (design-less) spec — where
        // planning actually adds value.
        SuggestThreshold::Thinness => {
            if !is_spec_thin(&req.description) {
                return None;
            }
            Some(format!(
                "{id} has no design section yet — `aida ultraplan {id}` to \
                 assemble a planning prompt before implementing."
            ))
        }
    }
}

/// TASK-304: render the ultraplan suggestion hint to stdout under the
/// shared pickup surfaces (queue next / work / list). No-op when the helper
/// returns `None`. trace:TASK-304 | ai:claude
pub(crate) fn print_ultraplan_suggestion_hint(
    project_root: &std::path::Path,
    req: &aida_core::Requirement,
) {
    if let Some(hint) = ultraplan_suggestion_hint(project_root, req) {
        println!();
        println!("  {} {}", "⤷".cyan(), hint);
    }
}

/// TASK-304: the `[ultraplan]` block `aida init` scaffolds into a new
/// project's `.aida/config.toml`. Ships `mode = "on-demand"` (preserves
/// current behavior) plus a comment explaining the never/on-demand/suggested
/// trade-off and why "frequently/auto-pull" isn't an option. Appended like
/// the `[forge]` section so all three init paths share one source of truth.
/// trace:TASK-304 | ai:claude
pub(crate) fn init_ultraplan_config_section() -> &'static str {
    "\n# trace:TASK-304 | ai:claude  (suggest_threshold: trace:TASK-697)\n\
     # Whether AIDA proactively suggests `aida ultraplan <SPEC>` for specs\n\
     # where planning would help. Planning is human/agent driven, so there\n\
     # is no \"auto-pull\" mode — only:\n\
     #   never      — `aida ultraplan` is disabled (refuses with a message)\n\
     #   on-demand  — current behavior: run `aida ultraplan SPEC` yourself\n\
     #   suggested  — pickup surfaces (/aida-pickup, queue work/list head)\n\
     #                hint when the head spec passes suggest_threshold\n\
     # suggest_threshold only applies in `suggested` mode. Heuristics:\n\
     #   spec-thinness       — (default) suggest when the spec is\n\
     #                         under-specified: no design section (## Proposed\n\
     #                         shape / Design / Approach / ...) yet a\n\
     #                         substantive body. Well-specified specs already\n\
     #                         carry their plan, so they stay quiet (SPIKE-8).\n\
     #   acceptance-bullets>N — (legacy) suggest when the `## Acceptance`\n\
     #                         checkbox count exceeds N. Note: bullet count\n\
     #                         anti-correlates with where planning helps.\n\
     [ultraplan]\n\
     mode = \"on-demand\"\n\
     suggest_threshold = \"spec-thinness\"\n"
}

// trace:TASK-760 | ai:claude
// The commented `[intake]` example block `aida init` scaffolds into a new
// project's `.aida/config.toml`. `aida intake` works with zero config (every
// knob has a safe default), so the block ships fully commented-out — it exists
// purely to make the policy knobs discoverable next to the other sections.
// Appended like the `[forge]` / `[ultraplan]` sections so all init paths
// share one source of truth. The example values mirror the defaults in
// `intake::IntakeConfig::default()` — keep them in lockstep.
pub(crate) fn init_intake_config_section() -> &'static str {
    "\n# Policy for `aida intake` — the headless advisor pass that reads open\n\
     # specs and proposes approve/reject/park/queue dispositions (propose-only\n\
     # by default; `--apply` executes). Every knob has a safe default, so this\n\
     # whole section is optional — uncomment a line only to change a default.\n\
     #\n\
     # disposition_bias — how aggressively the agent proposes approve:\n\
     #   approve-eligible  — (default) propose approve for every eligible\n\
     #                       spec; the propose-mode review is the filter\n\
     #   park-aligned      — approve only when a spec is BOTH eligible AND\n\
     #                       clearly aligned with project priorities; else\n\
     #                       park for a human\n\
     #   park-conservative — park whenever unsure\n\
     #\n\
     # do_not_approve_classes — requirement types the agent can NEVER propose\n\
     # approve for; they are fenced out of its candidate set entirely. The\n\
     # default is the strategic + knowledge-graph types. An empty list opens\n\
     # the gate (the propose-mode review still applies).\n\
     #\n\
     # on_apply — what `--apply` does after queuing approved specs:\n\
     #   queue — (default) stop at queuing; draining stays a separate,\n\
     #           explicit command\n\
     #   drain — chain straight into an implementer drain after queuing\n\
     #\n\
     # [intake]\n\
     # disposition_bias = \"approve-eligible\"\n\
     # do_not_approve_classes = [\"vision\", \"epic\", \"principle\", \"constraint\", \"decision\", \"term\"]\n\
     # on_apply = \"queue\"\n"
}

// trace:TASK-1021 trace:TASK-0429 | ai:claude
// The commented `[autopilot]` example block `aida init` scaffolds into a new
// project's `.aida/config.toml`. Same contract as the `[intake]` block above:
// every knob has a conservative default, so the whole section ships
// commented-out and exists purely to make the authority map discoverable next
// to the policy section it composes with.
//
// ONLY the nine action keys are documented, because only those parse today
// (`autopilot::ActionClass::parse` x `autopilot::Authority::parse`). The
// example values mirror `autopilot::AutopilotEnvelope::default()` — keep them
// in lockstep. The comment text is user-facing scaffolding, so it carries no
// SPEC-IDs and states plainly which surfaces honor the envelope today.
pub(crate) fn init_autopilot_config_section() -> &'static str {
    "\n# Authority envelope for advisor autopilot (`aida autopilot`). Autopilot is\n\
     # not a second disposition engine — it is a bounded-authority wrapper over\n\
     # the same grooming pass `[intake]` above configures. For each disposition\n\
     # the advisor proposes, the envelope decides one of: auto-execute, hold for\n\
     # a human, or escalate. Every knob has a conservative default, so this whole\n\
     # section is optional — uncomment a line only to change a default.\n\
     #\n\
     # One key per action class; each takes one of three values:\n\
     #   auto    — autopilot may perform the action on its own\n\
     #   propose — autopilot may only propose it; a human reviews before it runs\n\
     #   never   — autopilot may never perform it\n\
     #\n\
     # Defaults: the reversible, low-blast actions are `auto`; the two\n\
     # irreversible dispositions (approve, reject) are `propose`. An unknown key\n\
     # or an unknown value is ignored, leaving that action at its default.\n\
     #\n\
     # These knobs are a CEILING, not a blanket grant. Three bounds sit OUTSIDE\n\
     # the map and cannot be widened here:\n\
     #   * the candidate fence — which specs are touchable at all (`[intake]`);\n\
     #   * grounding — a call resting on judgment a freshly-started agent cannot\n\
     #     reconstruct from what is written down always escalates;\n\
     #   * the risk ceiling — high-risk and unknown-blast-radius work always\n\
     #     escalates.\n\
     # Context can only TIGHTEN the envelope, never widen it: an unattended run\n\
     # demotes every `propose` to `never` (nobody is there to be asked), and an\n\
     # active solo posture on keystone work demotes everything to `never`.\n\
     #\n\
     # What the envelope drives today: `aida autopilot inspect` — a read-only\n\
     # dry-run of what autopilot WOULD decide over the current groom candidates,\n\
     # recorded to a local audit log you can review (`aida autopilot audit`) and\n\
     # reverse (`aida autopilot challenge`) — and the draft approve-gate on\n\
     # `aida zen <spec>`. There is deliberately NO autopilot execution path on\n\
     # the grooming pass itself yet, so widening a key below changes those two\n\
     # surfaces and nothing else.\n\
     #\n\
     # [autopilot]\n\
     # approve = \"propose\"\n\
     # reject = \"propose\"\n\
     # dedupe = \"auto\"\n\
     # tag = \"auto\"\n\
     # queue = \"auto\"\n\
     # park = \"auto\"\n\
     # route = \"auto\"\n\
     # comment = \"auto\"\n\
     # ask = \"auto\"\n"
}

/// The `[store.sync] mirror_remotes` scaffold — commented. Automatic store
/// pushes go to `origin` only by default; explicit sync commands and an
/// opt-in `hub-mirror-sync` schedule can update configured mirror hubs.
// trace:TASK-1096 | ai:claude
// trace:BUG-1746 | ai:codex
pub(crate) fn init_store_mirror_config_section() -> &'static str {
    r#"
# Store-sync fan-out (STORY-760). By default automatic store pushes send
# the orphan store to `origin` only. Mirror hubs are also updated by
# `aida db sync --push`, `aida pull`, `aida remote mirror-sync`, or an
# opt-in `hub-mirror-sync` schedule job. Register that job hourly (strictly
# shorter than any drift guard interval) so transient failures get retries;
# no mirror-sync route job is intended, since `hub-drift-guard` escalates
# drift that persists. A non-fast-forward mirror-sync failure exits non-zero
# and must be reconciled deliberately; ordinary store writes remain best-effort.
# Check drift anytime with `aida remote status`.
#
# [store.sync]
# mirror_remotes = ["gitlab"]
"#
}

// trace:TASK-1522 | ai:antigravity
pub(crate) fn init_capture_config_section() -> &'static str {
    "\n# Effort-balance intent-capture target (TASK-1522): advisory floor on the\n\
     # share of newly completed specs with at least one traced test criterion.\n\
     # Reported by `aida criteria coverage` and `aida status`; not enforced.\n\
     #\n\
     # [capture]\n\
     # criterion_test_floor_pct = 50\n"
}

/// The `[worktree]` scaffold section. AIDA-created worktrees are expected to be
/// ready for builds, so recursive submodule initialization is default-on with a
/// visible opt-out.
// trace:BUG-899 | ai:codex
pub(crate) fn init_worktree_config_section() -> &'static str {
    "\n# Worktree creation. AIDA initializes recursive git submodules by default\n\
     # after creating or claiming a worktree, so repos that vendor dependencies\n\
     # through `.gitmodules` are build-ready before pickup succeeds. Set this\n\
     # false to skip the potentially-expensive init; AIDA will print the exact\n\
     # recovery command when `.gitmodules` is present.\n\
     [worktree]\n\
     init_submodules = true\n"
}

/// The `[worktree_pool]` scaffold section. Pooling is ON by default (TASK-985):
/// `aida session start` (and the agent-new / queue-work / orchestrator paths)
/// reuse a recycled warm worktree instead of `git worktree add`, keeping the
/// build cache warm across fan-out (~30× faster per-spec builds — see
/// docs/research/2026-06-29-warm-pool-build-delta.md).
// trace:TASK-985 | ai:claude
pub(crate) fn init_worktree_pool_config_section() -> &'static str {
    "\n# Worktree warm-pool (STORY-714). Pooling is ON by default: a session's\n\
     # worktree is acquired from a recycled pool and RESET (not deleted) on\n\
     # hand-back, so its compiled `target/` stays warm across fan-out. Escape\n\
     # hatches: set `enabled = false` here, pass `--no-pool` to a single\n\
     # `aida session start`, or pass `--remove` to `aida session end` to delete\n\
     # the worktree instead of returning it.\n\
     [worktree_pool]\n\
     enabled = true\n\
     # max_trees = 16   # cap on pooled worktrees (default 16)\n\
     # worktree_parent = \"../aida-worktrees\"   # nest every AIDA-created worktree\n\
     #                          # under ONE directory instead of scattering them as\n\
     #                          # siblings of the repo. An editor/agent folder-trust\n\
     #                          # grant inherits to children, so you trust this one\n\
     #                          # directory once and no future worktree prompts\n\
     #                          # again. Relative paths resolve against the repo\n\
     #                          # root; unset keeps the sibling layout (default)\n\
     # lease_ttl_secs = 21600   # a durable lease older than this (with no live\n\
     #                          # owner) is treated as EXPIRED and reclaimed by\n\
     #                          # the next acquire — guards against reservation\n\
     #                          # leaks when a session dies (default 6h)\n\
     #\n\
     # Pre-warm on create: set `prewarm_build = true` to kick off a backgrounded\n\
     # `cargo build` when a NEW pool tree is created, so its `target/` is warm\n\
     # before the first fanned agent builds in it (best-effort, non-blocking).\n\
     # Like every worktree-pool hook, this executes a build, so it is honored\n\
     # ONLY from your machine-global `~/.aida/config.toml` — never a checked-in\n\
     # repo config — and is ignored here.\n"
}

/// STORY-441: read `[archive] auto_after_days` from `.aida/config.toml`.
/// Returns `Some(days)` when the user has opted in, `None` when the key is
/// absent (auto-sweep stays off). Clamps below 7 to 7 with a stderr warning
/// — auto-archiving a freshly-shipped spec defeats the whole point.
/// Missing config file or unparseable TOML returns `None` silently (the
/// optional auto-sweep is a soft feature, not load-bearing).
/// trace:STORY-441 | ai:claude
pub(crate) fn read_archive_auto_after_days(project_root: &std::path::Path) -> Option<u64> {
    let path = config_path_for_project(project_root);
    let body = std::fs::read_to_string(&path).ok()?;
    let value: toml::Value = toml::from_str(&body).ok()?;
    let raw = value
        .get("archive")
        .and_then(|t| t.get("auto_after_days"))
        .and_then(|v| v.as_integer())?;
    let days = u64::try_from(raw).ok()?;
    if days < 7 {
        eprintln!(
            "{} [archive] auto_after_days = {days} clamped to minimum of 7 days \
             (archiving sooner hides freshly-shipped specs)",
            "Warning:".yellow().bold()
        );
        Some(7)
    } else {
        Some(days)
    }
}

/// BUG-783: read `[list] show_hidden_hints` from `.aida/config.toml`. Governs
/// whether the default `aida list` view footers the archived / deferred
/// view-tier counts ("(138 archived hidden — pass --all or --archived to see
/// them)"). Default is `false` — on an explicit open-work request those lines
/// are pure noise on every single invocation, and the tiers stay one flag away
/// (`--archived` / `--deferred` / `--all`). Set the key to `true` to bring the
/// nudges back. Missing config file / key / unparseable TOML → the default.
// trace:BUG-783 | ai:claude
pub(crate) fn read_list_show_hidden_hints(project_root: &std::path::Path) -> bool {
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return false;
    };
    value
        .get("list")
        .and_then(|t| t.get("show_hidden_hints"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// STORY-1644: read a `[section] key` value from `.aida/config.toml` via the
/// shared [`crate::kernel::read_project_config_value`] loader. Missing config
/// file / key / unparseable TOML → `None`.
// trace:STORY-1644 | ai:claude
fn project_config_key(
    project_root: &std::path::Path,
    section: &str,
    key: &str,
) -> Option<toml::Value> {
    crate::kernel::read_project_config_value(project_root)?
        .get(section)?
        .get(key)
        .cloned()
}

/// STORY-1644: read `[list] default_sort` (the `aida list` order when no
/// `--sort` is passed). `None` when unset.
// trace:STORY-1644 | ai:claude
pub(crate) fn read_list_default_sort(project_root: &std::path::Path) -> Option<String> {
    project_config_key(project_root, "list", "default_sort")
        .and_then(|v| v.as_str().map(str::to_string))
}

/// STORY-1644: read `[list] agent_limit` (the agent-mode `aida list` row
/// cap; 0 = unbounded). `None` when unset or not an integer.
// trace:STORY-1644 | ai:claude
pub(crate) fn read_list_agent_limit(project_root: &std::path::Path) -> Option<i64> {
    project_config_key(project_root, "list", "agent_limit").and_then(|v| v.as_integer())
}

/// STORY-1644: read `[history] limit` (the `aida history` default row
/// count). `None` when unset or not a positive integer.
// trace:STORY-1644 | ai:claude
pub(crate) fn read_history_limit(project_root: &std::path::Path) -> Option<i64> {
    project_config_key(project_root, "history", "limit")
        .and_then(|v| v.as_integer())
        .filter(|n| *n > 0)
}

/// STORY-1644: resolve `aida history`'s row count: an explicit `--limit`
/// wins, then env `AIDA_HISTORY_LIMIT`, then `[history] limit`, then the
/// built-in default. Non-positive / unparseable env values are ignored.
// trace:STORY-1644 | ai:claude
pub(crate) fn resolve_history_limit(
    flag: Option<usize>,
    env: Option<&str>,
    config: Option<i64>,
    default: usize,
) -> usize {
    if let Some(n) = flag {
        return n;
    }
    if let Some(n) = env
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
    {
        return n;
    }
    match config {
        Some(n) if n > 0 => n as usize,
        _ => default,
    }
}

/// STORY-1644: `aida history`'s effective row count for this invocation —
/// [`resolve_history_limit`] over the flag, `AIDA_HISTORY_LIMIT`, and the
/// project's `[history] limit`.
// trace:STORY-1644 | ai:claude
pub(crate) fn effective_history_limit(flag: Option<usize>) -> usize {
    let env = std::env::var("AIDA_HISTORY_LIMIT").ok();
    let config = if flag.is_some() {
        None
    } else {
        find_project_root()
            .ok()
            .and_then(|root| read_history_limit(&root))
    };
    resolve_history_limit(
        flag,
        env.as_deref(),
        config,
        crate::history_timeline::HISTORY_DEFAULT_LIMIT,
    )
}

/// STORY-717: read `[focus] out_of_scope` from `.aida/config.toml`. Governs the
/// focus-scope drift guard at work-start (off/warn/block). Missing config /
/// key / unparseable TOML falls back to the default (`warn`). The optional
/// `none`/`silent` and `refuse`/`hard` aliases are accepted by the parser.
// trace:STORY-717 | ai:claude
pub(crate) fn read_focus_out_of_scope_policy(
    project_root: &std::path::Path,
) -> focus::OutOfScopePolicy {
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return focus::OutOfScopePolicy::default();
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return focus::OutOfScopePolicy::default();
    };
    value
        .get("focus")
        .and_then(|t| t.get("out_of_scope"))
        .and_then(|v| v.as_str())
        .map(focus::parse_out_of_scope_policy)
        .unwrap_or_default()
}

/// STORY-717: derive a cheap "did you mean" focus suggestion for an out-of-scope
/// target — its `parent:` tag when present (the AIDA convention is
/// `parent:EPIC-NN`). Returns `None` when no cheap hint exists, in which case
/// the nudge just names the mismatch (per the spec: "if cheap; else just name
/// the mismatch").
// trace:STORY-717 | ai:claude
pub(crate) fn suggested_focus_for(target: &aida_core::Requirement) -> Option<String> {
    target
        .tags
        .iter()
        .find_map(|t| t.strip_prefix("parent:"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// STORY-717: the focus-scope drift guard. At a work-START moment, if the
/// current worktree has a focus set (STORY-706) and `target` is OUTSIDE that
/// focus's transitive subtree, apply the configured `[focus] out_of_scope`
/// policy. `force` ALWAYS overrides. Membership reuses the cache's
/// `descendant_ids` closure (TASK-955) — the same subtree the focus read-scope
/// uses — rather than re-walking the hierarchy. An unresolvable focus spec
/// skips the guard rather than blocking real work. The subtree is read with a
/// strict cache refresh, and a cache or refresh error fails the start (fail
/// closed) instead of judging scope on a stale graph; a `Block` policy also
/// returns `Err` on a genuine out-of-scope start.
// trace:STORY-717 | ai:claude
// trace:BUG-1670 | ai:claude
pub(crate) fn focus_scope_guard(
    project_root: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
    target: &aida_core::Requirement,
    force: bool,
) -> Result<()> {
    // No focus set in this worktree → no check.
    let Some(focus_ref) = focus::resolve_focus(project_root) else {
        return Ok(());
    };
    let policy = read_focus_out_of_scope_policy(project_root);
    // Cheap exit before touching the cache when the guard is muted anyway.
    if policy == focus::OutOfScopePolicy::Off {
        return Ok(());
    }
    // The focus label must still resolve to a real spec; if it no longer does,
    // don't block work (the read-scope path warns about a dangling focus).
    let Some(focus_req) = backend.get_requirement_by_spec_id(&focus_ref)? else {
        return Ok(());
    };
    // REUSE the TASK-955 subtree closure (includes the root) for membership.
    // Strict: this gates a write, so a child added or re-parented by another
    // writer since the last cache refresh must be classified on the current
    // graph, not on a snapshot served while the cache is being written.
    // trace:BUG-1670 | ai:claude
    let subtree = backend.descendant_ids_strict(&focus_req.id)?;
    let in_scope = focus::is_in_focus_scope(&target.id, &subtree);
    match focus::decide_focus_action(policy, in_scope, force) {
        focus::FocusGuardAction::Proceed => Ok(()),
        focus::FocusGuardAction::Warn => {
            let core = focus::out_of_scope_message(
                &target.display_id(),
                &focus_req.display_id(),
                suggested_focus_for(target).as_deref(),
            );
            eprintln!(
                "{} {} (--force to silence)",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                core
            );
            Ok(())
        }
        focus::FocusGuardAction::Block => {
            let core = focus::out_of_scope_message(
                &target.display_id(),
                &focus_req.display_id(),
                suggested_focus_for(target).as_deref(),
            );
            anyhow::bail!("{} (pass --force to override)", core)
        }
    }
}

/// STORY-717: focus guard for callers that don't already hold a backend
/// (`queue work`, `agent new --spec`). Resolves `spec` against a cache-backed
/// backend, then defers to [`focus_scope_guard`]. Skips early (no backend work)
/// when no focus is set; best-effort on store/spec resolution. A `Block` policy
/// still propagates the `Err`.
// trace:STORY-717 | ai:claude
pub(crate) fn focus_scope_guard_for_spec(
    project_root: &std::path::Path,
    spec: &str,
    force: bool,
) -> Result<()> {
    // Fast path: no focus → skip the (potentially cache-rebuilding) backend open.
    if focus::resolve_focus(project_root).is_none() {
        return Ok(());
    }
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Ok(());
    };
    let Ok(backend) = advance_backend(&store_path) else {
        return Ok(());
    };
    let Some(target) = backend.get_requirement_by_spec_id(spec)? else {
        return Ok(());
    };
    focus_scope_guard(project_root, &backend, &target, force)
}

// BUG-653: `aida agent new --spec <epic>` used to dead-end. The launch path
// routes through `session_start` -> `preflight_spec_status`, which refuses a
// Draft spec with "transition it to Approved first". But an epic's status is a
// read-only rollup of its children (BUG-626) -- `aida edit <epic> --status
// approved` is rejected -- so that advice sends the operator to a command that
// will refuse. And you don't implement an epic directly anyway; work happens on
// its children. Detect the epic up front and give an epic-appropriate message
// instead of routing into the implement-readiness gate. Non-epic specs (incl.
// genuinely-Draft tasks/stories) fall through untouched, so their correct
// "transition to Approved" refusal is preserved. trace:BUG-653 | ai:claude
pub(crate) fn epic_agent_new_guard(project_root: &std::path::Path, spec: &str) -> Result<()> {
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Ok(());
    };
    let Ok(backend) = advance_backend(&store_path) else {
        return Ok(());
    };
    let Some(target) = backend.get_requirement_by_spec_id(spec)? else {
        return Ok(());
    };
    match epic_agent_new_refusal(&target.req_type, &target.display_id()) {
        Some(message) => anyhow::bail!("{message}"),
        None => Ok(()),
    }
}

/// BUG-653: pure decision half of [`epic_agent_new_guard`] — returns the
/// epic-appropriate refusal message when the resolved `--spec` is an epic, or
/// `None` for every other type (so non-epic specs fall through to the existing
/// readiness gate untouched). Pure so the message + the type gate are
/// unit-testable without a store fixture.
// trace:BUG-653 | ai:claude
pub(crate) fn epic_agent_new_refusal(
    req_type: &RequirementType,
    display_id: &str,
) -> Option<String> {
    if !matches!(req_type, RequirementType::Epic) {
        return None;
    }
    Some(format!(
        "{display_id}'s status is a read-only rollup of its children -- you don't implement an \
         epic directly.\n  \
         Pick an approved child and launch on that:\n    \
         aida list --parent {display_id} --status approved\n  \
         Or open a design/advisor session scoped to the epic in a focused worktree (no --spec):\n    \
         aida worktree enter {display_id}   # then a plain `aida agent new ...` in that worktree"
    ))
}

// BUG-1701: route a drain-mode spec away from the interactive lane. `aida agent
// new --spec <ID>` spawns a vendor TUI and BLOCKS until that TUI exits; a TUI
// does not exit when its turn ends, so an orchestrator that dispatched a
// drain-groomed spec here stranded both the seat and itself. The one-shot lane
// already exists -- `aida do <SPEC>` routes by groomed execution mode, and for
// drain that is `aida queue work <SPEC> --auto-complete`, which runs headless
// and returns an exit status. Nothing steered callers there, so this guard does.
// Sits beside the epic and focus-scope guards: AFTER the dry-preview returns (a
// preview is never blocked) and BEFORE any worktree/lease/status side effect.
// trace:BUG-1701 | ai:claude
pub(crate) fn drain_mode_agent_new_guard(
    project_root: &std::path::Path,
    spec: &str,
    force: bool,
    holds_caller: bool,
) -> Result<()> {
    if force {
        return Ok(());
    }
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Ok(());
    };
    let Ok(backend) = advance_backend(&store_path) else {
        return Ok(());
    };
    // Fail OPEN on a lookup failure as well as a miss. `?` here would abort the
    // launch when the store is unreadable, which is the opposite of this guard's
    // contract: it exists to REDIRECT a drain-groomed spec, never to become a new
    // way for every launch in the fleet to fail. An independent review of PR #2249
    // caught the `?`. trace:BUG-1701 | ai:claude
    let Ok(Some(target)) = backend.get_requirement_by_spec_id(spec) else {
        return Ok(());
    };
    match drain_mode_agent_new_refusal(target.execution_mode, &target.display_id(), holds_caller) {
        Some(message) => anyhow::bail!("{message}"),
        None => Ok(()),
    }
}

/// Pure decision half of [`drain_mode_agent_new_guard`]: the refusal message for a
/// spec groomed `execution_mode = drain`, or `None` for every other mode (and for
/// an ungroomed spec, which must keep working -- the fail-open default).
///
/// Only `drain` is refused. The other modes legitimately want a human-attended
/// seat, which is exactly what this lane provides; refusing them would take away
/// the only lane they have.
///
/// `holds_caller` distinguishes the foreground launch (which blocks the caller on
/// the child TUI -- the BUG-1701 stall) from `--bg` (which detaches, so the lane
/// is still wrong but for the pipeline reason alone). Stating only what is true of
/// the lane actually being refused keeps the message trustworthy.
// trace:BUG-1701 | ai:claude
pub(crate) fn drain_mode_agent_new_refusal(
    mode: Option<aida_core::ExecutionMode>,
    display_id: &str,
    holds_caller: bool,
) -> Option<String> {
    if mode != Some(aida_core::ExecutionMode::Drain) {
        return None;
    }
    // The first line must stand alone: in agent mode only the first line becomes
    // the `error:` summary. And `aida do` must be the FIRST backtick-quoted
    // `aida ...` command in the whole message, because that is what agent mode
    // lifts into `help:` -- an unbackticked recommendation silently loses to a
    // backticked one further down. trace:BUG-1701 | ai:claude
    let lane = if holds_caller {
        "which spawns an interactive seat and holds your caller until that seat's TUI exits -- \
         and a TUI does not exit when its turn ends"
    } else {
        "which runs neither CI, the reviewer phase, nor the merge for you"
    };
    Some(format!(
        "{display_id} is groomed `execution_mode = drain` -- use `aida do {display_id}`, not this \
         lane, {lane}.\n  \
         `aida do` routes by the groomed mode; for drain it runs \
         `aida queue work {display_id} --auto-complete`, which is headless, runs to completion, \
         and returns an exit status.\n  \
         To sit in this spec interactively anyway (to debug it by hand), pass --force."
    ))
}

/// STORY-564: read `[zen] auto_exit` from `.aida/config.toml`. Returns the
/// operator's persistent preference for whether a clean standalone-`--zen`
/// finish auto-exits (`true`, the default) or always pauses (`false`).
/// Missing config / key / unparseable TOML → `true` (the new default
/// behavior). The per-invocation `--pause-always` flag (→ `AIDA_ZEN_PAUSE_ALWAYS`)
/// overrides this toward pausing; this is the standing preference when the
/// flag isn't passed. trace:STORY-564 | ai:claude
pub(crate) fn read_zen_auto_exit(project_root: &std::path::Path) -> bool {
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return true;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return true;
    };
    value
        .get("zen")
        .and_then(|t| t.get("auto_exit"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

/// STORY-564: should the standalone-`--zen` finish *pause* (vs auto-exit)
/// because the operator asked it to? True when `--pause-always` was passed
/// (propagated as `AIDA_ZEN_PAUSE_ALWAYS=1`) OR `[zen] auto_exit = false` is
/// configured. Either is the operator electing to drive grab-next by hand.
/// trace:STORY-564 | ai:claude
pub(crate) fn zen_pause_always_in_force(project_root: &std::path::Path) -> bool {
    std::env::var(zen::ZEN_PAUSE_ALWAYS_ENV).as_deref() == Ok("1")
        || !read_zen_auto_exit(project_root)
}

/// STORY-569: read `[zen] review_brief_agent` from `.aida/config.toml` — the
/// mailbox target for the clean-finish build→review handoff. Missing config /
/// key / unparseable TOML → the default `advisor`. An explicitly empty string
/// disables the handoff. trace:STORY-569 | ai:claude
pub(crate) fn read_zen_review_brief_agent(project_root: &std::path::Path) -> Option<String> {
    let default = || Some("advisor".to_string());
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return default();
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return default();
    };
    match value
        .get("zen")
        .and_then(|t| t.get("review_brief_agent"))
        .and_then(|v| v.as_str())
    {
        None => default(),
        Some(s) if s.trim().is_empty() => None,
        Some(s) => Some(s.trim().to_string()),
    }
}

/// STORY-441: opt-out env var matching the `AIDA_AUTO_BUMP` shape.
pub(crate) fn auto_archive_enabled() -> bool {
    match std::env::var("AIDA_AUTO_ARCHIVE") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// STORY-441: run the same `--older-than N days --status completed,rejected`
/// sweep that `aida archive --older-than` exposes, but as a side-effect of
/// `aida pull` once the auto-bump has finished. Off by default — gated on
/// `[archive] auto_after_days` being set. Best-effort: any error is printed
/// as a warning, never fails the pull. trace:STORY-441 | ai:claude
pub(crate) fn maybe_auto_archive_sweep(
    project_root: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
    quiet: bool,
) {
    if !auto_archive_enabled() {
        return;
    }
    let Some(days) = read_archive_auto_after_days(project_root) else {
        return;
    };
    let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
    let statuses = ["Completed", "Rejected"];
    let mut to_archive: Vec<aida_core::Requirement> = Vec::new();
    // cache-tolerant-read: selection only — each candidate's YAML object is
    // re-read and its eligibility re-decided inside the store write lock
    // (`bulk_update_atomically`) before anything is written.
    // trace:BUG-1671 | ai:claude
    for s in &statuses {
        let filter = aida_core::ListFilter {
            status: Some((*s).to_string()),
            archive: aida_core::ArchiveFilter::NonArchivedOnly,
            ..Default::default()
        };
        let Ok(rows) = backend.list_summaries(&filter) else {
            continue;
        };
        for row in rows {
            let stale = chrono::DateTime::parse_from_rfc3339(&row.modified_at)
                .map(|dt| dt.with_timezone(&chrono::Utc) < cutoff)
                .unwrap_or(false);
            if !stale {
                continue;
            }
            // BUG-1664: `row` may come from a stale cache snapshot. The
            // object read is authoritative, so re-check status and age on it,
            // not only the archive flag.
            // trace:BUG-1664 | ai:claude
            if let Ok(Some(req)) = backend.get_requirement(&row.id) {
                if archive_cmd::archive_sweep_still_eligible(&req, &statuses, cutoff) {
                    to_archive.push(req);
                }
            }
        }
    }
    if to_archive.is_empty() {
        return;
    }
    let now = chrono::Utc::now();
    crate::sweep_test_hook::fire(backend.path());
    // BUG-1671: re-decide each candidate on the object read INSIDE the store
    // write lock, so a spec reopened between the pass above and this write is
    // skipped instead of being reverted by the whole-object write. Collapses
    // the old commit-per-spec loop into one commit as well.
    // trace:BUG-1671 | ai:claude
    let count = backend
        .bulk_update_atomically(&to_archive, "chore(archive)", |req| {
            if !archive_cmd::archive_sweep_still_eligible(req, &statuses, cutoff) {
                return false;
            }
            req.archived = true;
            req.archived_at = Some(now);
            req.modified_at = now;
            true
        })
        .map(|report| report.written.len())
        .unwrap_or(0);
    if !quiet && count > 0 {
        println!(
            "  {} {count} spec(s) older than {days}d (auto-sweep, opt out via AIDA_AUTO_ARCHIVE=0)",
            "auto-archived:".cyan()
        );
    }
}
