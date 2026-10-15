//! Plan-file follow-up extraction, interface/test-plan capture and follow-up task filing (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// Comment prefix written to a spec once its plan followups have been
/// processed. Both extraction paths check for it first, so whichever runs
/// first (`queue done` interactively, or the auto-bump non-interactively)
/// wins and the other skips — declines are never silently re-filed.
pub(crate) const FOLLOWUPS_MARKER: &str = "[aida:followups]";

/// True when followup automation is disabled via `AIDA_AUTO_FOLLOWUPS`
/// (mirrors the `AIDA_AUTO_BUMP` opt-out shape).
pub(crate) fn auto_followups_disabled() -> bool {
    matches!(
        std::env::var("AIDA_AUTO_FOLLOWUPS")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "false" | "0" | "no" | "off"
    )
}

/// BUG-655: content-level dedup for auto-followups. The [`FOLLOWUPS_MARKER`]
/// comment is a *per-spec* fast-path, but it is not a hard guarantee: a plan
/// carrying a `## Followups` section can be committed into more than one slice
/// worktree (STORY-712 shipped via 4 slice PRs), and two independent commits
/// that each reach completion before the other's marker comment has synced
/// will *both* fire the filing — double-filing the same bullets. The robust
/// guarantee is content-level: before filing a bullet as a child TASK, check
/// whether a child TASK with the SAME parent spec AND the SAME (trimmed,
/// case-insensitive) title already exists, and skip it if so. Child specs are
/// real, queryable store rows, so this survives across commits/agents/PRs in a
/// way the single marker comment does not.
///
/// `existing_children` is the `(parent_spec_id, title)` set already present in
/// the store. Pure + total so it is unit-testable in isolation.
pub(crate) fn followup_already_filed(
    existing_children: &[(String, String)],
    parent: &str,
    bullet: &str,
) -> bool {
    let norm = |s: &str| s.trim().to_ascii_lowercase();
    let want = norm(bullet);
    existing_children
        .iter()
        .any(|(p, title)| p == parent && norm(title) == want)
}

/// Parse the plan paths a [`FOLLOWUPS_MARKER`] comment records. The marker's
/// first line is `[aida:followups] extracted from <p1>, <p2>, ...`; everything
/// after `extracted from` up to the newline is the comma-separated relative
/// plan-path list. Returns the trimmed, non-empty paths (empty for any comment
/// that is not a followups marker, or a marker with no `extracted from` clause).
/// Pure + total so it is unit-testable in isolation.
// trace:BUG-656 | ai:claude
pub(crate) fn parse_extracted_plans_from_marker(comment: &str) -> Vec<String> {
    let Some(rest) = comment.strip_prefix(FOLLOWUPS_MARKER) else {
        return Vec::new();
    };
    let Some(first_line) = rest.lines().next() else {
        return Vec::new();
    };
    let Some(paths) = first_line.trim().strip_prefix("extracted from ") else {
        return Vec::new();
    };
    paths
        .split(',')
        .map(|p| normalize_recorded_plan_path(p.trim()))
        .filter(|p| !p.is_empty())
        .collect()
}

/// A plan path read back from a followup marker or a `followup-src:` tag, in
/// the `/` form [`plan_rel_path`] now writes. Versions before BUG-1648 wrote
/// `docs/plans\x.md` on Windows; normalizing on read keeps those dedup-able.
// trace:BUG-1648 | ai:claude
pub(crate) fn normalize_recorded_plan_path(recorded: &str) -> String {
    recorded.replace('\\', "/")
}

/// BUG-656: the cross-store stable signature. Given the relative plan paths a
/// completing spec owns and the set of plan paths already extracted (parsed from
/// every spec's [`FOLLOWUPS_MARKER`] comments), return only the plans not yet
/// extracted. Empty result ⇒ every owned plan's followups were already filed by
/// an earlier completion, so the whole pass is a no-op. Pure + total.
// trace:BUG-656 | ai:claude
pub(crate) fn plans_pending_extraction(
    owned_rel_paths: &[String],
    already_extracted: &std::collections::HashSet<String>,
) -> Vec<String> {
    owned_rel_paths
        .iter()
        .filter(|p| !already_extracted.contains(*p))
        .cloned()
        .collect()
}

/// BUG-656: secondary global-title guard. The plan-path signature stops a plan
/// being re-extracted, but the SAME followup text copied into a *distinct*
/// sibling plan (a different owning spec, a different path) would still slip
/// through. `existing_titles` is the normalized titles of every followup TASK
/// already filed across the store; a bullet whose trimmed/case-insensitive title
/// matches one of them has already been filed somewhere and is skipped. Pure +
/// total.
// trace:BUG-656 | ai:claude
pub(crate) fn followup_filed_anywhere(existing_titles: &[String], bullet: &str) -> bool {
    let want = bullet.trim().to_ascii_lowercase();
    existing_titles.iter().any(|t| *t == want)
}

/// BUG-1625: does `store` already hold `bullet` as a filed followup of
/// `parent`? True when a spec with the same trimmed, case-insensitive title is
/// a child of `parent` or carries the [`FOLLOWUP_SRC_TAG_PREFIX`] provenance
/// tag of `source_plan` — the plan this bullet came from. BUG-1633: a
/// same-titled spec filed from a DIFFERENT plan is not this followup.
/// Used to classify a failed `aida add` as "already filed" instead of
/// "declined". Pure + total.
// trace:BUG-1625 trace:BUG-1633 | ai:claude
pub(crate) fn followup_filed_in_store(
    store: &RequirementsStore,
    parent: &str,
    bullet: &str,
    source_plan: &str,
) -> bool {
    use aida_core::models::RelationshipType;
    let want = bullet.trim().to_ascii_lowercase();
    let child_ids: std::collections::HashSet<uuid::Uuid> = store
        .get_requirement_by_spec_id(parent)
        .map(|p| {
            p.relationships
                .iter()
                .filter(|r| r.rel_type == RelationshipType::Parent)
                .map(|r| r.target_id)
                .collect()
        })
        .unwrap_or_default();
    store.requirements.iter().any(|r| {
        r.title.trim().to_ascii_lowercase() == want
            && (child_ids.contains(&r.id)
                || r.tags.iter().any(|t| {
                    // Tags written before BUG-1648 may carry `\`.
                    // trace:BUG-1648 | ai:claude
                    t.strip_prefix(FOLLOWUP_SRC_TAG_PREFIX)
                        .is_some_and(|p| normalize_recorded_plan_path(p) == source_plan)
                }))
    })
}

/// BUG-680: tag prefix that records the source plan path on a followup TASK the
/// auto-followup path files. The BUG-656 marker comment records the extraction
/// on the *completing spec*, but that comment can be lost or arrive unsynced
/// (two commits completing before either marker syncs — the same window BUG-655
/// flagged), and it says nothing once the followup itself has shipped and been
/// archived away from the parent. Stamping the provenance on the child spec
/// makes it durable and queryable: the followup carries its own origin, so a
/// later re-extraction can recognise "this bullet already shipped from this
/// plan" straight from the store even when the parent's marker is gone. Flat,
/// colon-namespaced provenance tag (like `parent:`, `batch:`) per the tag
/// conventions.
// trace:BUG-680 | ai:claude
pub(crate) const FOLLOWUP_SRC_TAG_PREFIX: &str = "followup-src:";

pub(crate) const FOLLOWUP_META_PROSE_OPENERS: &[&str] =
    &["consider", "maybe", "possibly", "optionally"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FollowupSkip {
    PointerToExistingSpec(String),
    MetaProse(&'static str),
}

pub(crate) fn followup_strip_markdown_emphasis(text: &str) -> &str {
    text.trim()
        .trim_start_matches('*')
        .trim_start_matches('_')
        .trim_end_matches('*')
        .trim_end_matches('_')
        .trim()
}

// trace:BUG-1142 | ai:codex
pub(crate) fn followup_meta_prose_opener(text: &str) -> Option<&'static str> {
    let stripped = followup_strip_markdown_emphasis(text);
    let lower = stripped.to_ascii_lowercase();
    FOLLOWUP_META_PROSE_OPENERS.iter().copied().find(|opener| {
        lower == *opener
            || lower.starts_with(&format!("{opener}:"))
            || lower.starts_with(&format!("{opener} "))
    })
}

// trace:BUG-1142 | ai:codex
pub(crate) fn followup_existing_spec_pointer(
    store: &aida_core::models::RequirementsStore,
    text: &str,
) -> Option<String> {
    let re = regex::Regex::new(r"\b[A-Z][A-Z0-9]+(?:-\d+){1,3}\b").ok()?;
    let found = re.find_iter(text).find_map(|m| {
        let token = m.as_str();
        store
            .get_requirement_by_spec_id(token)
            .map(|req| req.display_id())
    });
    found
}

// trace:BUG-1142 | ai:codex
pub(crate) fn classify_plan_followup(
    store: &aida_core::models::RequirementsStore,
    text: &str,
) -> Option<FollowupSkip> {
    if let Some(id) = followup_existing_spec_pointer(store, text) {
        return Some(FollowupSkip::PointerToExistingSpec(id));
    }
    followup_meta_prose_opener(text).map(FollowupSkip::MetaProse)
}

/// BUG-680: a followup bullet already filed from the SAME source plan that has
/// since reached a terminal status (Completed / Rejected — the work shipped or
/// was rejected). `filed_from_plan` is `(recorded_plan_path, title, spec_id,
/// is_terminal)` for every spec carrying a [`FOLLOWUP_SRC_TAG_PREFIX`] tag;
/// `owned_plans` is the set of source plans currently being extracted. Returns
/// the id of a terminal spec previously filed from one of `owned_plans` whose
/// title matches `bullet` — re-filing it would create a second open spec for a
/// followup that already ran its course, so the caller skips and links to the
/// returned id. Match is trim + case-insensitive on the title; the plan path is
/// exact (both sides are the store-relative path). Pure + total so it is
/// unit-testable in isolation.
// trace:BUG-680 | ai:claude
pub(crate) fn followup_shipped_from_plan<'a>(
    filed_from_plan: &'a [(String, String, String, bool)],
    owned_plans: &std::collections::HashSet<String>,
    bullet: &str,
) -> Option<&'a str> {
    let want = bullet.trim().to_ascii_lowercase();
    filed_from_plan
        .iter()
        .find(|(plan, title, _, terminal)| {
            *terminal && owned_plans.contains(plan) && title.trim().to_ascii_lowercase() == want
        })
        .map(|(_, _, id, _)| id.as_str())
}

/// Find the docs/plans/ files that *belong to* `spec_id` — the id appears
/// in the `# Plan:` title line or the `Specs:` header line. Cross-references
/// in a `## Related` section don't count (that plan owns a different spec).
pub(crate) fn find_plan_files_for_spec(
    project_root: &std::path::Path,
    spec_id: &str,
) -> Vec<std::path::PathBuf> {
    let plans_dir = project_root.join("docs/plans");
    let Ok(entries) = std::fs::read_dir(&plans_dir) else {
        return Vec::new();
    };
    let needle = regex::Regex::new(&format!(r"\b{}\b", regex::escape(spec_id)));
    let Ok(needle) = needle else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Only the header zone (title + `Specs:` line) confers ownership.
        let owns = content
            .lines()
            .take(15)
            .any(|l| (l.starts_with("# ") || l.starts_with("Specs:")) && needle.is_match(l));
        if owns {
            hits.push(path);
        }
    }
    hits.sort();
    hits
}

/// Parse the `## Followups` section of a plan: collect the column-0
/// `- `/`* ` bullets until the next `##` header. Leading marker and a
/// trailing period are stripped; placeholder/empty bullets are dropped.
/// TASK-431: is this `## Followups` bullet a "no followups" sentinel
/// (`None`, `N/A`, `NA`, `Nothing`, `(none)`, optionally followed by an
/// em-dash / colon / spaced-dash explanation) rather than a real followup?
/// Conservative — only when the sentinel is the WHOLE leading clause, so a
/// genuine bullet like "None of these handlers do X" is kept. trace:TASK-431
pub(crate) fn followup_is_sentinel(text: &str) -> bool {
    let lower = text.trim().to_ascii_lowercase();
    let unparen = lower.trim_start_matches('(').trim_end_matches(')').trim();
    // Leading clause = everything before the first explanation separator.
    let head = unparen
        .split([':', '—'])
        .next()
        .unwrap_or(unparen)
        .split(" - ")
        .next()
        .unwrap_or(unparen)
        .trim();
    matches!(
        head,
        "none" | "n/a" | "na" | "nothing" | "nothing here" | "nothing to do"
    )
}

pub(crate) fn parse_plan_followups(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_section = false;
    let mut in_fence = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(h) = trimmed.strip_prefix("## ") {
            in_section = h.to_ascii_lowercase().contains("followup");
            continue;
        }
        if !in_section {
            continue;
        }
        // Column-0 bullets only — nested detail bullets are not followups.
        let bullet = line.strip_prefix("- ").or_else(|| line.strip_prefix("* "));
        if let Some(b) = bullet {
            let text = b.trim().trim_end_matches('.').trim().to_string();
            // BUG-104: skip only a literal template placeholder (a bullet that
            // STARTS with `<`, e.g. `<describe the followup>`) — not any bullet
            // that merely contains `<` (a real followup like "support `Vec<T>`"
            // or "fix the `a < b` guard" was being silently dropped).
            if text.is_empty()
                || text.starts_with('<')
                || text.starts_with("file as")
                || followup_is_sentinel(&text)
            {
                continue;
            }
            out.push(text);
        }
    }
    out
}

/// TASK-95: parse the `## Critical Files` section — the column-0 bullets'
/// backtick-quoted paths. The flat must-touch blast radius.
pub(crate) fn parse_plan_critical_files(content: &str) -> Vec<String> {
    let path_re = regex::Regex::new(r"`([A-Za-z0-9_][A-Za-z0-9_./\-]*\.[A-Za-z0-9]+)`").unwrap();
    let mut out: Vec<String> = Vec::new();
    let mut in_section = false;
    let mut in_fence = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(h) = trimmed.strip_prefix("## ") {
            in_section = h.to_ascii_lowercase().contains("critical files");
            continue;
        }
        if !in_section {
            continue;
        }
        if line.starts_with("- ") || line.starts_with("* ") {
            for cap in path_re.captures_iter(line) {
                let p = cap[1].to_string();
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// TASK-95: parse the `## Verification` section — the body of its first
/// fenced code block, i.e. the executable definition of done.
pub(crate) fn parse_plan_verification(content: &str) -> Option<String> {
    let mut in_section = false;
    let mut in_fence = false;
    let mut body = String::new();
    let mut captured = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if !in_section {
            if let Some(h) = trimmed.strip_prefix("## ") {
                in_section = h.to_ascii_lowercase().contains("verification");
            }
            continue;
        }
        if trimmed.starts_with("## ") {
            break; // reached the next section without a closed fence
        }
        if trimmed.starts_with("```") {
            if in_fence {
                captured = true;
                break;
            }
            in_fence = true;
            continue;
        }
        if in_fence {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(line);
        }
    }
    if captured && !body.trim().is_empty() {
        Some(body)
    } else {
        None
    }
}

/// TASK-95: build the plan brief for `spec_id` — locate the owning
/// docs/plans/ file and extract its Critical Files, Followups, and
/// Verification sections. Returns `None` when no plan file owns the spec
/// (the graceful no-op `aida queue work` falls back to).
pub(crate) fn discover_plan_context(
    project_root: &std::path::Path,
    spec_id: &str,
) -> Option<session_manifest::PlanContext> {
    // BUG-105: when more than one plan file owns the spec, merge them all
    // instead of silently using only the first. Critical-files and followups
    // are unioned (dedup, order-preserving); verification scripts are
    // concatenated; plan_file lists every contributing path.
    let plan_files = find_plan_files_for_spec(project_root, spec_id);
    let mut rels: Vec<String> = Vec::new();
    let mut critical_files: Vec<String> = Vec::new();
    let mut followups: Vec<String> = Vec::new();
    let mut verifications: Vec<String> = Vec::new();
    for plan_file in &plan_files {
        let Ok(content) = std::fs::read_to_string(plan_file) else {
            continue;
        };
        // `/`-separated on every OS, like the followup marker. trace:BUG-1648 | ai:claude
        rels.push(plan_rel_path(plan_file, project_root));
        for c in parse_plan_critical_files(&content) {
            if !critical_files.contains(&c) {
                critical_files.push(c);
            }
        }
        for f in parse_plan_followups(&content) {
            if !followups.contains(&f) {
                followups.push(f);
            }
        }
        if let Some(v) = parse_plan_verification(&content) {
            verifications.push(v);
        }
    }
    if rels.is_empty() {
        return None;
    }
    Some(session_manifest::PlanContext {
        plan_file: rels.join(", "),
        critical_files,
        followups,
        verification: if verifications.is_empty() {
            None
        } else {
            Some(verifications.join("\n\n"))
        },
    })
}

/// BUG-1633: test-only stand-in for the followup `aida add` subprocess.
/// Receives `(project_root, parent, title, source_plan)` and returns the new
/// spec id, or `None` to simulate a failed add.
// trace:BUG-1633 | ai:claude
#[cfg(test)]
pub(crate) type FollowupAddHook =
    Box<dyn FnMut(&std::path::Path, &str, &str, Option<&str>) -> Option<String>>;

#[cfg(test)]
thread_local! {
    pub(crate) static FOLLOWUP_ADD_HOOK: std::cell::RefCell<Option<FollowupAddHook>> =
        const { std::cell::RefCell::new(None) };
}

/// Self-invoke `aida add` to file one followup as a child TASK of
/// `parent_spec`. Always passes `--force-parent` — the parent is Done or
/// Completed by the time we file, and we explicitly want the children
/// regardless. When `source_plan` is set, stamps the plan's provenance as a
/// [`FOLLOWUP_SRC_TAG_PREFIX`] tag so a later re-extraction can dedup against
/// this followup even after it ships (BUG-680). Returns the new spec id on
/// success.
pub(crate) fn aida_subcmd_add_followup_task(
    project_root: &std::path::Path,
    parent_spec: &str,
    title: &str,
    source_plan: Option<&str>,
) -> Option<String> {
    // BUG-1633: tests substitute the `aida add` subprocess (the test binary is
    // not `aida`). trace:BUG-1633 | ai:claude
    #[cfg(test)]
    if let Some(result) = FOLLOWUP_ADD_HOOK.with(|h| {
        h.borrow_mut()
            .as_mut()
            .map(|f| f(project_root, parent_spec, title, source_plan))
    }) {
        return result;
    }
    let aida = aida_exe_path();
    let mut args: Vec<String> = vec![
        "add".into(),
        "--type".into(),
        "task".into(),
        "--status".into(),
        "approved".into(),
        "--priority".into(),
        "low".into(),
        "--parent".into(),
        parent_spec.into(),
        "--force-parent".into(),
        "--title".into(),
        title.into(),
    ];
    // BUG-680: durable source-plan provenance on the child, so re-filing after
    // this followup ships is a no-op straight from the store. trace:BUG-680
    if let Some(plan) = source_plan {
        args.push("--tags".into());
        args.push(format!("{FOLLOWUP_SRC_TAG_PREFIX}{plan}"));
    }
    let out = std::process::Command::new(&aida)
        .current_dir(project_root)
        .args(&args)
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "{} followup `aida add` failed: {}",
            "Warning:".yellow().bold(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    parse_spec_id_from_add_output(&String::from_utf8_lossy(&out.stdout))
}

/// True when interface-change capture is disabled via
/// `AIDA_CAPTURE_INTERFACE_CHANGES=0|false|no`. Mirrors
/// [`auto_followups_disabled`]: the env-opt-out keeps the close-checkpoint out
/// of unattended drains that don't want the prompt. trace:STORY-542 | ai:claude
pub(crate) fn capture_interface_changes_disabled() -> bool {
    matches!(
        std::env::var("AIDA_CAPTURE_INTERFACE_CHANGES")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "0" | "false" | "no"
    )
}

/// Prompt for the surface-delta lines on one interface (cli/mcp/tui/other) at a
/// TTY: one line per `Enter`, blank line ends the surface. Returns the
/// collected lines. trace:STORY-542 | ai:claude
pub(crate) fn prompt_interface_surface(label: &str, example: &str) -> Vec<String> {
    use std::io::Write;
    let mut lines = Vec::new();
    eprintln!(
        "  {} {} change(s)? One per line, blank line when done.",
        "→".cyan(),
        label.bold()
    );
    eprintln!("    {} {}", "e.g.".dimmed(), example.dimmed());
    loop {
        eprint!("    > ");
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
            break;
        }
        let trimmed = answer.trim();
        if trimmed.is_empty() {
            break;
        }
        lines.push(trimmed.to_string());
    }
    lines
}

/// Capture the spec's user-facing interface changes at close (`aida queue
/// done`) — the deterministic Layer-1 source for the operator digest
/// (STORY-541 / STORY-542). Precedence:
///
/// 1. `--no-interface-change` ⇒ record nothing (the spec stays out of the
///    operator digest), no prompt.
/// 2. Any `--interface-{cli,mcp,tui,other}` flag ⇒ use exactly those lines,
///    no prompt (the deterministic / agent path).
/// 3. Otherwise, at a TTY and when `interactive` (not `--yes`) and not
///    env-disabled ⇒ prompt per surface (derive-and-confirm is future work;
///    today the implementer types the lines).
/// 4. Non-interactive with no flags ⇒ leave `interface_changes` untouched
///    (empty), exactly as a no-impact spec.
///
/// Idempotent: if the spec already carries a non-empty `interface_changes`
/// (e.g. `/aida-pr` populated it earlier), and no flags override it, it is left
/// alone. Best-effort — any failure returns `Ok(())` so it never breaks
/// `queue done`. trace:STORY-542 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn capture_interface_changes(
    storage: &Storage,
    target: &aida_core::Requirement,
    display_id: &str,
    flag_cli: &[String],
    flag_mcp: &[String],
    flag_tui: &[String],
    flag_other: &[String],
    no_interface_change: bool,
    interactive: bool,
) -> Result<()> {
    let req_id = target.id;
    let any_flag = !flag_cli.is_empty()
        || !flag_mcp.is_empty()
        || !flag_tui.is_empty()
        || !flag_other.is_empty();

    // Resolve the changes to record. `None` ⇒ leave the spec untouched.
    let changes: Option<aida_core::InterfaceChanges> = if no_interface_change {
        // Explicit "no user-facing change" — record an empty marker so a
        // re-run / the auto-bump path both treat this spec as decided and the
        // operator digest skips it. We persist `Some(empty)` to distinguish
        // "decided: nothing" from "never asked".
        Some(aida_core::InterfaceChanges::default())
    } else if any_flag {
        Some(aida_core::InterfaceChanges {
            cli: flag_cli.to_vec(),
            mcp: flag_mcp.to_vec(),
            tui: flag_tui.to_vec(),
            other: flag_other.to_vec(),
        })
    } else {
        // No flags: only prompt at a TTY, interactively, and not opted out.
        let at_tty = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
        if !interactive || !at_tty || capture_interface_changes_disabled() {
            return Ok(());
        }
        // Idempotency: don't re-prompt if a non-empty capture already exists.
        let store = storage.load()?;
        if let Some(r) = store.requirements.iter().find(|r| r.id == req_id) {
            if r.interface_changes
                .as_ref()
                .is_some_and(|ic| !ic.is_empty())
            {
                return Ok(());
            }
        }
        eprintln!();
        eprintln!(
            "{} How does {} change the user-facing interface? (feeds the operator digest)",
            "→".cyan().bold(),
            display_id.bold()
        );
        eprintln!(
            "  {} no user-facing change (clippy/refactor/test)? Just press Enter through each.",
            "·".dimmed()
        );
        let ic = aida_core::InterfaceChanges {
            cli: prompt_interface_surface("CLI", "aida foo — new command"),
            mcp: prompt_interface_surface("MCP", "queue_add — now advisor-gated"),
            tui: prompt_interface_surface("TUI", "press `g` — jump to graph view"),
            other: prompt_interface_surface("Other", "REST /digest endpoint added"),
        };
        Some(ic)
    };

    let Some(changes) = changes else {
        return Ok(());
    };

    let now = chrono::Utc::now();
    let was_empty = changes.is_empty();
    let summary = if was_empty {
        "no user-facing interface change".to_string()
    } else {
        let mut parts = Vec::new();
        if !changes.cli.is_empty() {
            parts.push(format!("{} cli", changes.cli.len()));
        }
        if !changes.mcp.is_empty() {
            parts.push(format!("{} mcp", changes.mcp.len()));
        }
        if !changes.tui.is_empty() {
            parts.push(format!("{} tui", changes.tui.len()));
        }
        if !changes.other.is_empty() {
            parts.push(format!("{} other", changes.other.len()));
        }
        parts.join(", ")
    };

    // Per-spec compare-and-swap, no whole-store write. trace:BUG-1612 | ai:claude
    storage.update_spec_atomically(target, |r| {
        // Store `Some(empty)` for the explicit no-change case so it reads as
        // "decided"; store the populated set otherwise.
        r.interface_changes = Some(changes.clone());
        r.modified_at = now;
    })?;

    if was_empty {
        println!("  {} interface changes: none recorded", "·".dimmed());
    } else {
        println!(
            "  {} interface changes captured for {} ({})",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            display_id.bold(),
            summary.dimmed()
        );
    }
    Ok(())
}

/// True when verification-step capture is disabled via
/// `AIDA_AUTO_TEST_PLAN_CAPTURE=0|false|no`. Mirrors [`auto_followups_disabled`]
/// and [`capture_interface_changes_disabled`] — the env-opt-out keeps the
/// close-checkpoint prompt out of unattended drains.
// trace:STORY-698 | ai:claude
pub(crate) fn capture_test_plan_disabled() -> bool {
    matches!(
        std::env::var("AIDA_AUTO_TEST_PLAN_CAPTURE")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "0" | "false" | "no"
    )
}

/// What [`capture_test_plan`] should do with the verification steps, given the
/// resolved flag / TTY / opt-out state. Split out as a pure decision so the
/// precedence rules are unit-testable without a TTY.
// trace:STORY-698 | ai:claude
#[derive(Debug, PartialEq)]
pub(crate) enum TestPlanCapture {
    /// Leave `test_coverage_notes` untouched (no prompt, no write).
    Skip,
    /// Store exactly these joined steps (the flag / deterministic path).
    Record(String),
    /// Prompt the builder at the TTY for the steps.
    Prompt,
}

/// Precedence for verification-step capture at `aida queue done`:
///
/// 1. Any `--test-plan STEP` flag ⇒ record exactly those steps, no prompt.
/// 2. `--no-test-plan` ⇒ skip (leave any existing notes untouched), no prompt.
/// 3. Otherwise, at a TTY, interactive (not `--yes`), not env-disabled, and
///    with nothing already captured ⇒ prompt.
/// 4. Any other case (non-interactive, no TTY, opted out, already set) ⇒ skip.
///
/// A `--test-plan` flag overrides an existing value (explicit intent); the
/// interactive path never re-prompts once a value is set.
// trace:STORY-698 | ai:claude
pub(crate) fn decide_test_plan_capture(
    flag_steps: &[String],
    no_test_plan: bool,
    interactive: bool,
    at_tty: bool,
    disabled: bool,
    already_set: bool,
) -> TestPlanCapture {
    if !flag_steps.is_empty() {
        return TestPlanCapture::Record(flag_steps.join("\n"));
    }
    if no_test_plan || !interactive || !at_tty || disabled || already_set {
        return TestPlanCapture::Skip;
    }
    TestPlanCapture::Prompt
}

/// Prompt at a TTY for the verification steps the builder ran: one step per
/// `Enter`, blank line ends. Mirrors [`prompt_interface_surface`]. Returns the
/// collected lines.
// trace:STORY-698 | ai:claude
pub(crate) fn prompt_test_plan_steps() -> Vec<String> {
    use std::io::Write;
    let mut lines = Vec::new();
    eprintln!(
        "  {} verification steps you ran? One per line, blank line when done.",
        "→".cyan()
    );
    eprintln!(
        "    {} {}",
        "e.g.".dimmed(),
        "cargo test -p aida-cli · manual: aida queue done at a TTY".dimmed()
    );
    loop {
        eprint!("    > ");
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
            break;
        }
        let trimmed = answer.trim();
        if trimmed.is_empty() {
            break;
        }
        lines.push(trimmed.to_string());
    }
    lines
}

/// Capture the verification steps the builder actually ran at close (`aida
/// queue done`) — the implementation audit trail surfaced in the PR body
/// (STORY-698). Stored in `implementation_info.test_coverage_notes` (no new
/// model field, no cache migration — design LOCKED 2026-07-01). Steps are
/// joined newline-separated.
///
/// Best-effort — any failure returns `Ok(())` so it never breaks `queue done`.
/// Idempotent: an existing non-empty `test_coverage_notes` is left alone unless
/// a `--test-plan` flag explicitly overrides it.
// trace:STORY-698 | ai:claude
pub(crate) fn capture_test_plan(
    storage: &Storage,
    target: &aida_core::Requirement,
    display_id: &str,
    flag_steps: &[String],
    no_test_plan: bool,
    interactive: bool,
) -> Result<()> {
    let req_id = target.id;
    // Cheap pre-checks (flags / opt-out) decide most cases without a load.
    let at_tty = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let disabled = capture_test_plan_disabled();

    // Idempotency only matters for the interactive path; check it there.
    let already_set = |storage: &Storage| -> bool {
        storage
            .load()
            .ok()
            .and_then(|store| {
                store
                    .requirements
                    .iter()
                    .find(|r| r.id == req_id)
                    .and_then(|r| r.implementation_info.as_ref())
                    .and_then(|info| info.test_coverage_notes.as_ref())
                    .map(|notes| !notes.trim().is_empty())
            })
            .unwrap_or(false)
    };

    // Resolve without touching the store when the flags already decide it.
    let decision = if !flag_steps.is_empty() {
        decide_test_plan_capture(
            flag_steps,
            no_test_plan,
            interactive,
            at_tty,
            disabled,
            false,
        )
    } else if no_test_plan || !interactive || !at_tty || disabled {
        TestPlanCapture::Skip
    } else {
        decide_test_plan_capture(
            flag_steps,
            no_test_plan,
            interactive,
            at_tty,
            disabled,
            already_set(storage),
        )
    };

    let notes = match decision {
        TestPlanCapture::Skip => return Ok(()),
        TestPlanCapture::Record(joined) => joined,
        TestPlanCapture::Prompt => {
            eprintln!();
            eprintln!(
                "{} How did you verify {}? (recorded as the PR's audit trail)",
                "→".cyan().bold(),
                display_id.bold()
            );
            eprintln!("  {} nothing to record? Just press Enter.", "·".dimmed());
            let lines = prompt_test_plan_steps();
            if lines.is_empty() {
                return Ok(());
            }
            lines.join("\n")
        }
    };

    let now = chrono::Utc::now();
    let step_count = notes.lines().filter(|l| !l.trim().is_empty()).count();
    // Per-spec compare-and-swap, no whole-store write. trace:BUG-1612 | ai:claude
    storage.update_spec_atomically(target, |r| {
        let info = r
            .implementation_info
            .get_or_insert_with(aida_core::ImplementationInfo::default);
        info.test_coverage_notes = Some(notes.clone());
        r.modified_at = now;
    })?;

    println!(
        "  {} verification steps captured for {} ({} step{})",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        display_id.bold(),
        step_count,
        if step_count == 1 { "" } else { "s" }
    );
    Ok(())
}

/// A plan's project-relative path in the form the store records it: `/`
/// separated on every OS. The followup marker, the plan-path dedup set and
/// the `followup-src:` tag compare these strings, so a Windows clone that
/// wrote `docs/plans\x.md` would neither match what a Unix clone filed nor
/// the `docs/plans/x.md` shape everything else uses. The agent-brief
/// `.pending` sentinel (BUG-466) stores its keys the same way.
// trace:BUG-1648 | ai:claude
pub(crate) fn plan_rel_path(path: &std::path::Path, project_root: &std::path::Path) -> String {
    let Ok(rel) = path.strip_prefix(project_root) else {
        return path.display().to_string();
    };
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Extract the Followups section of any plan owned by `spec_id` and file
/// the accepted bullets as child TASKs. Idempotent via [`FOLLOWUPS_MARKER`].
///
/// `interactive` drives a per-bullet `[y/N/skip]` prompt; when false (the
/// auto-bump / `--yes` path) every bullet is filed. Best-effort throughout —
/// any failure logs a warning and returns `Ok(())` so it never breaks the
/// `queue done` / `aida pull` flow it hangs off. trace:TASK-96 | ai:claude
pub(crate) fn extract_plan_followups(
    storage: &Storage,
    project_root: &std::path::Path,
    spec_id: &str,
    display_id: &str,
    interactive: bool,
) -> Result<()> {
    if auto_followups_disabled() {
        return Ok(());
    }
    let store = storage.load()?;
    let Some(req) = store.get_requirement_by_spec_id(spec_id) else {
        return Ok(());
    };
    // Already processed — whichever path ran first owns the outcome.
    if req
        .comments
        .iter()
        .any(|c| c.content.starts_with(FOLLOWUPS_MARKER))
    {
        return Ok(());
    }

    let plan_files = find_plan_files_for_spec(project_root, spec_id);
    if plan_files.is_empty() {
        return Ok(());
    }

    // BUG-656: plan-path stable signature — the cross-store guarantee. Build the
    // set of plan paths whose followups have ALREADY been extracted (by this
    // spec, a re-completion, or any sibling spec that also owns the plan), then
    // drop owned plans already in it. The per-spec marker comment above only
    // suppresses re-runs of the SAME spec; this keys dedup on the plan path so a
    // plan whose header lists several specs is extracted exactly once. Without
    // it, completing each EPIC-0428 design task re-extracted the shared plans
    // and re-filed their followups (TASK-1019-1032). trace:BUG-656 | ai:claude
    let already_extracted: std::collections::HashSet<String> = store
        .requirements
        .iter()
        .flat_map(|r| r.comments.iter())
        .flat_map(|c| parse_extracted_plans_from_marker(&c.content))
        .collect();
    let owned_rel_paths: Vec<String> = plan_files
        .iter()
        .map(|p| plan_rel_path(p, project_root))
        .collect();
    let pending: std::collections::HashSet<String> =
        plans_pending_extraction(&owned_rel_paths, &already_extracted)
            .into_iter()
            .collect();
    if pending.is_empty() {
        // Every owned plan's followups were already filed by an earlier
        // completion — re-completion / sibling-spec completion is a no-op.
        return Ok(());
    }
    let plan_files: Vec<std::path::PathBuf> = plan_files
        .into_iter()
        .filter(|p| pending.contains(&plan_rel_path(p, project_root)))
        .collect();

    // BUG-655: content-level dedup set — the `(parent_spec, title)` of every
    // child TASK the parent already has. The marker comment above is the
    // fast-path, but a plan committed by more than one slice PR can fire the
    // filing twice before the first marker syncs; this set is the real
    // guarantee, since the children are durable store rows. We grow it in the
    // filing loop too so a plan that lists the same bullet twice is also a
    // no-op the second time.
    let mut existing_children: Vec<(String, String)> = {
        use aida_core::models::RelationshipType;
        req.relationships
            .iter()
            .filter(|r| r.rel_type == RelationshipType::Parent)
            .filter_map(|r| store.get_requirement_by_id(&r.target_id))
            .map(|child| (spec_id.to_string(), child.title.clone()))
            .collect()
    };

    // BUG-656: secondary global-title guard. Collect the normalized titles of
    // every followup TASK already filed across the store — the child TASKs of
    // any spec that carries a FOLLOWUPS_MARKER. If the same bullet text was
    // copied into a distinct sibling plan (a different owning spec, a path the
    // plan-path signature above won't match), this stops it being re-filed under
    // the new parent. We grow it in the filing loop too. trace:BUG-656 | ai:claude
    let mut global_filed_titles: Vec<String> = {
        use aida_core::models::RelationshipType;
        let mut titles = Vec::new();
        for r in &store.requirements {
            if !r
                .comments
                .iter()
                .any(|c| c.content.starts_with(FOLLOWUPS_MARKER))
            {
                continue;
            }
            for rel in r
                .relationships
                .iter()
                .filter(|rel| rel.rel_type == RelationshipType::Parent)
            {
                if let Some(child) = store.get_requirement_by_id(&rel.target_id) {
                    titles.push(child.title.trim().to_ascii_lowercase());
                }
            }
        }
        titles
    };

    // BUG-680: shipped-followup guard. Every spec that a prior run filed carries
    // a `followup-src:<plan>` tag recording its origin plan; collect
    // (plan, title, id, is_terminal) for each so a bullet already filed from one
    // of THIS spec's plans that has since shipped (Completed) or been rejected is
    // not re-filed as a fresh open spec. This is the durable, store-backed
    // guarantee the parent's marker comment can't make: it survives the marker
    // being lost/unsynced and survives the followup being archived away from the
    // parent. `owned_plans` is the store-relative path set we match against.
    // trace:BUG-680 | ai:claude
    let owned_plans: std::collections::HashSet<String> = owned_rel_paths.iter().cloned().collect();
    let filed_from_plan: Vec<(String, String, String, bool)> = store
        .requirements
        .iter()
        .flat_map(|r| {
            let title = r.title.clone();
            let id = r.display_id();
            let terminal = matches!(
                r.status,
                RequirementStatus::Completed | RequirementStatus::Rejected
            );
            r.tags
                .iter()
                .filter_map(|t| t.strip_prefix(FOLLOWUP_SRC_TAG_PREFIX))
                // trace:BUG-1648 | ai:claude
                .map(move |plan| {
                    (
                        normalize_recorded_plan_path(plan),
                        title.clone(),
                        id.clone(),
                        terminal,
                    )
                })
        })
        .collect();

    // Collect + dedupe followup bullets across every owning plan file. Each
    // bullet keeps the first plan it appeared in as its source, so a filed
    // followup can be stamped with its origin plan (BUG-680).
    let mut followups: Vec<(String, String)> = Vec::new(); // (bullet, source plan)
    let mut sources: Vec<String> = Vec::new();
    for path in &plan_files {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let parsed = parse_plan_followups(&content);
        if parsed.is_empty() {
            continue;
        }
        let rel = plan_rel_path(path, project_root);
        sources.push(rel.clone());
        for f in parsed {
            if !followups.iter().any(|(b, _)| b == &f) {
                followups.push((f, rel.clone()));
            }
        }
    }
    if followups.is_empty() {
        return Ok(());
    }

    println!();
    println!(
        "{} {} plan followup(s) found for {} ({})",
        "→".cyan().bold(),
        followups.len(),
        display_id.bold(),
        sources.join(", ").dimmed()
    );

    let mut filed: Vec<(String, String)> = Vec::new(); // (new_spec, title)
    let mut declined: Vec<String> = Vec::new();
    let mut deduped: Vec<String> = Vec::new(); // BUG-655: already-filed bullets
    let mut shipped: Vec<(String, String)> = Vec::new(); // BUG-680: (bullet, existing id)
    let mut pointer_skips: Vec<(String, String)> = Vec::new(); // BUG-1142: (bullet, existing id)
    let mut meta_skips: Vec<(String, &'static str)> = Vec::new(); // BUG-1142: (bullet, opener)
    let mut skip_rest = false;

    for (followup, source_plan) in &followups {
        match classify_plan_followup(&store, followup) {
            Some(FollowupSkip::PointerToExistingSpec(id)) => {
                pointer_skips.push((followup.clone(), id));
                continue;
            }
            Some(FollowupSkip::MetaProse(opener)) => {
                meta_skips.push((followup.clone(), opener));
                continue;
            }
            None => {}
        }
        // BUG-680: a bullet already filed from one of this spec's plans that has
        // since shipped (Completed) or been rejected — re-filing it would open a
        // duplicate for work that already ran its course. Skip and link to the
        // shipped spec. Checked first so the audit trail attributes it to the
        // shipped guard rather than the generic already-filed one.
        if let Some(existing_id) =
            followup_shipped_from_plan(&filed_from_plan, &owned_plans, followup)
        {
            shipped.push((followup.clone(), existing_id.to_string()));
            continue;
        }
        // BUG-655: a bullet whose title already exists as a child of this
        // parent has already been filed (this run, a prior run, or a sibling
        // commit's run) — skip it without prompting, so a plan landed by
        // multiple PRs cannot double-file. This is the cross-commit guarantee
        // the per-spec marker comment cannot make.
        // BUG-656: also skip a bullet whose title was already filed as a
        // followup TASK ANYWHERE in the store — the same text copied into a
        // sibling plan owned by a different spec must not become a duplicate.
        if followup_already_filed(&existing_children, spec_id, followup)
            || followup_filed_anywhere(&global_filed_titles, followup)
        {
            deduped.push(followup.clone());
            continue;
        }

        let accept = if skip_rest {
            false
        } else if interactive {
            eprint!("  File as TASK? [y/N/skip] {}\n  > ", followup.bold());
            use std::io::Write;
            let _ = std::io::stderr().flush();
            let mut answer = String::new();
            if std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut answer).is_err() {
                skip_rest = true;
                false
            } else {
                match answer.trim().to_ascii_lowercase().as_str() {
                    "y" | "yes" => true,
                    "s" | "skip" => {
                        skip_rest = true;
                        false
                    }
                    _ => false,
                }
            }
        } else {
            true
        };

        if accept {
            match aida_subcmd_add_followup_task(project_root, spec_id, followup, Some(source_plan))
            {
                Some(new_id) => {
                    // Record the title so a later identical bullet in the same
                    // plan is recognised as already-filed (BUG-655), both
                    // per-parent and globally (BUG-656).
                    existing_children.push((spec_id.to_string(), followup.clone()));
                    global_filed_titles.push(followup.trim().to_ascii_lowercase());
                    filed.push((new_id, followup.clone()));
                }
                // BUG-1625: a failed add is not automatically a decline — the
                // followup may already exist in the (now fresher) store, filed
                // by another clone. Re-read and record it as already-filed so
                // pull output never reports an existing task as declined.
                // trace:BUG-1625 | ai:claude
                None => {
                    // BUG-1633: only THIS plan's provenance tag counts.
                    // trace:BUG-1633 | ai:claude
                    let exists = storage
                        .load()
                        .map(|fresh| {
                            followup_filed_in_store(&fresh, spec_id, followup, source_plan)
                        })
                        .unwrap_or(false);
                    if exists {
                        deduped.push(followup.clone());
                    } else {
                        declined.push(followup.clone());
                    }
                }
            }
        } else {
            declined.push(followup.clone());
        }
    }

    // Marker comment — records the outcome so re-runs and the other
    // extraction path both skip, and the user can see what was declined.
    let mut marker = format!("{} extracted from {}", FOLLOWUPS_MARKER, sources.join(", "));
    if filed.is_empty() {
        marker.push_str("\nfiled 0 task(s)");
    } else {
        let ids: Vec<&str> = filed.iter().map(|(id, _)| id.as_str()).collect();
        marker.push_str(&format!(
            "\nfiled {} task(s): {}",
            filed.len(),
            ids.join(", ")
        ));
    }
    if !declined.is_empty() {
        marker.push_str(&format!("\ndeclined {}:", declined.len()));
        for d in &declined {
            marker.push_str(&format!("\n  - {d}"));
        }
    }
    if !deduped.is_empty() {
        // BUG-655: bullets skipped because an identical child TASK already
        // existed — recorded so the audit trail shows the double-file was
        // prevented, not silently dropped.
        marker.push_str(&format!("\nskipped {} already-filed:", deduped.len()));
        for d in &deduped {
            marker.push_str(&format!("\n  - {d}"));
        }
    }
    if !shipped.is_empty() {
        // BUG-680: bullets skipped because a followup filed from the same plan
        // already shipped (Completed/Rejected) — linked to the existing spec so
        // the audit trail shows the re-file was prevented, not lost.
        marker.push_str(&format!("\nskipped {} already-shipped:", shipped.len()));
        for (bullet, id) in &shipped {
            marker.push_str(&format!("\n  - {bullet} → {id}"));
        }
    }
    if !pointer_skips.is_empty() {
        marker.push_str(&format!(
            "\nskipped {} pointer-to-existing-spec:",
            pointer_skips.len()
        ));
        for (bullet, id) in &pointer_skips {
            marker.push_str(&format!("\n  - {bullet} → {id}"));
        }
    }
    if !meta_skips.is_empty() {
        marker.push_str(&format!("\nskipped {} meta-prose:", meta_skips.len()));
        for (bullet, opener) in &meta_skips {
            marker.push_str(&format!("\n  - {bullet} (opener: {opener})"));
        }
    }
    let now = chrono::Utc::now();
    let author = get_default_author();
    // Per-spec compare-and-swap, no whole-store write. trace:BUG-1612 | ai:claude
    let _ = storage.update_spec_atomically(req, |r| {
        r.comments
            .push(Comment::new(author.clone(), marker.clone()));
        r.modified_at = now;
    });

    if filed.is_empty() {
        println!("  {} no followups filed", "·".dimmed());
    } else {
        for (id, title) in &filed {
            println!(
                "  {} {} {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                id.bold(),
                title.dimmed()
            );
        }
    }
    if !declined.is_empty() {
        println!(
            "  {} {} followup(s) not filed — logged on {} for later review",
            "·".dimmed(),
            declined.len(),
            display_id
        );
    }
    if !deduped.is_empty() {
        println!(
            "  {} {} followup(s) already filed as child task(s) — skipped",
            "·".dimmed(),
            deduped.len(),
        );
    }
    if !shipped.is_empty() {
        for (bullet, id) in &shipped {
            println!(
                "  {} already shipped as {} — skipped {}",
                "·".dimmed(),
                id.bold(),
                bullet.dimmed(),
            );
        }
    }
    if !pointer_skips.is_empty() {
        for (bullet, id) in &pointer_skips {
            println!(
                "  {} pointer-to-existing-spec {} — skipped {}",
                "·".dimmed(),
                id.bold(),
                bullet.dimmed(),
            );
        }
    }
    if !meta_skips.is_empty() {
        for (bullet, opener) in &meta_skips {
            println!(
                "  {} meta-prose opener `{}` — skipped {}",
                "·".dimmed(),
                opener,
                bullet.dimmed(),
            );
        }
    }

    Ok(())
}
