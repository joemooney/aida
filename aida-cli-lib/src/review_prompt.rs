// trace:TASK-3428 | ai:codex
use crate::review_gates::{review_coverage_from_commits, ReviewCoverage};
use crate::{
    detect_forge_from_origin, extract_acceptance_section, find_project_root, git_log_messages,
    parse_review_scope, pr_base_head, stale_base_note, ReviewForge, Storage,
};
use anyhow::{Context, Result};
use colored::Colorize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// trace:STORY-67 | ai:claude
pub(crate) fn generate_review_prompt(
    storage: &Storage,
    specs_csv: Option<&str>,
    pr: Option<u64>,
    forge_override: Option<&str>,
    write_path: Option<&str>,
) -> Result<()> {
    let store = storage.load()?;

    // Resolve the spec list. Preference order: --specs explicit,
    // --pr range parse, error if neither.
    // trace:BUG-1434 | ai:claude
    let (coverage, change_body, header_subtitle, staleness_note) = if let Some(csv) = specs_csv {
        let ids: Vec<String> = csv
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if ids.is_empty() {
            anyhow::bail!("--specs was empty after splitting on commas");
        }
        let label = format!("Specs: {}", ids.join(", "));
        (
            ReviewCoverage {
                delivered: ids,
                ..Default::default()
            },
            String::new(),
            label,
            None,
        )
    } else if let Some(pr_n) = pr {
        let project_root = find_project_root()?;
        let forge = forge_override
            .and_then(ReviewForge::parse)
            .or_else(|| detect_forge_from_origin(&project_root))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "couldn't detect forge from origin URL — pass --forge github|gitlab"
                )
            })?;
        let (base, head) = pr_base_head(&project_root, forge, pr_n)?;
        let messages = git_log_messages(&project_root, &base, &head)?;
        let (title, body) = match crate::forge::forge_for_kind(&project_root, forge.forge_kind())
            .change_title_and_body(pr_n)
        {
            Ok(metadata) => metadata,
            Err(err) => {
                eprintln!("warning: could not read change title or slice scope: {err:#}");
                (String::new(), String::new())
            }
        };
        let coverage = prompt_coverage(&messages, &title);
        if coverage.delivered.is_empty() && coverage.slice_refs.is_empty() {
            anyhow::bail!(
                "no `(REQ-ID)` or `(Refs REQ-ID)` trailers found in the title or {}..{} ({} commits inspected)",
                base,
                head,
                messages.len()
            );
        }
        let label = match forge {
            ReviewForge::GitHub => format!("PR #{} — branch `{}`", pr_n, head),
            ReviewForge::GitLab => format!("MR !{} — branch `{}`", pr_n, head),
        };
        let note = stale_base_note(&project_root, &head, &base);
        (coverage, body, label, note)
    } else {
        anyhow::bail!("pass --specs <CSV> or --pr <N>");
    };

    // Compose the markdown.
    let mut out = String::new();
    out.push_str("# Review Prompt\n\n");
    out.push_str(&header_subtitle);
    out.push_str("\n\n");
    if let Some(note) = &staleness_note {
        out.push_str("## Staleness\n\n");
        out.push_str(note);
        out.push('\n');
    }
    // trace:TASK-3814 | ai:codex
    // trace:STORY-2913 | ai:claude — every prompt quotes the active list.
    out.push_str(&crate::review_blocking::prompt(&find_project_root()?)?);
    let (spec_count, missing) = append_spec_checklist(&mut out, &store, &coverage, &change_body);

    out.push_str(review_prompt_test_commands_section());

    out.push_str("## Decide\n\n");
    if !coverage.slice_refs.is_empty() {
        out.push_str(
            "Base the verdict on the applicable checklist items and defects in the diff. \
             After merge, completion applies only to specs with completing trailers.\n\n",
        );
    } else {
        out.push_str(
            "If every item above passes: approve and merge (`gh pr merge --squash` / \
         `glab mr merge --squash`), then mark each linked req `completed`.\n\n",
        );
    }
    out.push_str(
        "If any applicable item fails: request changes with specifics tied to the spec_id, \
         so the contributor can address them by id (not by paraphrase).\n",
    );

    if let Some(path) = write_path {
        std::fs::write(path, &out)
            .with_context(|| format!("failed to write review prompt to {}", path))?;
        eprintln!(
            "{} review prompt written to {} ({} spec{}{})",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
            path,
            spec_count,
            if spec_count == 1 { "" } else { "s" },
            if missing == 0 {
                String::new()
            } else {
                format!(", {} missing in store", missing)
            },
        );
    } else {
        print!("{}", out);
    }
    Ok(())
}

// trace:BUG-2683 | ai:codex
fn prompt_coverage(messages: &[String], title: &str) -> ReviewCoverage {
    let mut coverage = review_coverage_from_commits(messages);
    for (ids, title_ids) in [
        (
            &mut coverage.delivered,
            crate::pr_ship::extract_trailing_spec_ids_from_subject(title),
        ),
        (
            &mut coverage.slice_refs,
            crate::pr_ship::extract_trailing_refs_spec_ids_from_subject(title),
        ),
    ] {
        for id in title_ids {
            let id = crate::spec_arg::canonical(&id);
            if !ids
                .iter()
                .any(|existing| aida_core::id_identity::same_id(existing, &id))
            {
                ids.push(id);
            }
        }
    }
    coverage.slice_refs.retain(|id| {
        !coverage
            .delivered
            .iter()
            .any(|delivered| aida_core::id_identity::same_id(delivered, id))
    });
    coverage
}

fn append_spec_checklist(
    out: &mut String,
    store: &aida_core::RequirementsStore,
    coverage: &ReviewCoverage,
    change_body: &str,
) -> (usize, usize) {
    out.push_str("## What to verify\n\n");
    let mut seen = BTreeSet::new();
    let mut missing = 0;
    for (ids, slice) in [(&coverage.delivered, false), (&coverage.slice_refs, true)] {
        for id in ids {
            // TASK-72: PR/MR pseudo-IDs are forge references, not checklist specs.
            if parse_review_scope(id).is_some() {
                continue;
            }
            let Some(req) = store
                .requirements
                .iter()
                .find(|req| req.spec_id.as_deref() == Some(id))
                .or_else(|| {
                    store
                        .requirements
                        .iter()
                        .find(|req| crate::queue_cmd::spec_matches(req, id))
                })
            else {
                let _ = writeln!(out, "### {id}\n\n_(not found in store)_\n");
                missing += 1;
                continue;
            };
            // Completion wins even when the two trailers use different IDs for the spec.
            if !seen.insert(req.id) {
                continue;
            }
            let display = req.spec_id.as_deref().unwrap_or(id);
            let _ = writeln!(out, "### {display} — {}\n", req.title);
            let acceptance = extract_acceptance_section(&req.description);
            if slice {
                let criteria =
                    crate::criteria::parse_acceptance_criteria(display, &req.description);
                if let Some(scope) = parse_slice_scope(change_body, req, &criteria) {
                    out.push_str("#### Declared slice scope\n\nReview covered criteria using their usual evidence requirements. Record deferred criteria as shown below and exclude their deferral from defects. Inspect the entire diff for defects.\n\n");
                    for criterion in criteria {
                        let status = match scope[criterion.label.to_ascii_lowercase().as_str()] {
                            Some(target) => {
                                format!("not-applicable-because deferred to `{target}`")
                            }
                            None => "covered — verify evidence".to_string(),
                        };
                        let _ =
                            writeln!(out, "- {}: {} — {status}", criterion.label, criterion.text);
                    }
                    out.push('\n');
                    append_implementer_approach(out, &req.comments);
                    continue;
                }
                out.push_str("**Slice scope undeclared or invalid.** Review the diff's own claims and inspect it for defects. List uncovered criteria as informational, not blocking.\n\n");
                if acceptance.is_none() && !criteria.is_empty() {
                    for criterion in criteria {
                        let _ = writeln!(out, "- {}: {}", criterion.label, criterion.text);
                    }
                    out.push('\n');
                    append_implementer_approach(out, &req.comments);
                    continue;
                }
            }
            match acceptance {
                Some(body) => {
                    out.push_str(&body);
                    out.push_str("\n\n");
                }
                None => out.push_str("_(no `## Acceptance` / `## Verify` section in description — review against the requirement title and description.)_\n\n"),
            }
            append_implementer_approach(out, &req.comments);
        }
    }
    (seen.len() + missing, missing)
}

/// A declaration must assign each parsed criterion exactly once. Partial,
/// ambiguous or stale declarations use the same contract as an undeclared slice.
fn parse_slice_scope<'a>(
    body: &'a str,
    req: &aida_core::Requirement,
    criteria: &[crate::criteria::Criterion],
) -> Option<BTreeMap<String, Option<&'a str>>> {
    let labels: BTreeSet<_> = criteria
        .iter()
        .map(|ac| ac.label.to_ascii_lowercase())
        .collect();
    if labels.is_empty() || labels.len() != criteria.len() {
        return None;
    }
    let mut scope = BTreeMap::new();
    for line in body
        .lines()
        .filter_map(|line| line.strip_prefix("Slice-Scope:"))
    {
        let mut fields = line.split_whitespace();
        let Some(spec) = fields.next() else { continue };
        if !crate::queue_cmd::spec_matches(req, spec) {
            continue;
        }
        let label = fields.next()?.to_ascii_lowercase();
        let target = match (fields.next(), fields.next(), fields.next()) {
            (Some("covered"), None, None) => None,
            (Some("deferred"), Some(target), None)
                if target
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_./#".contains(&b))
                    && target.bytes().any(|b| b.is_ascii_alphanumeric()) =>
            {
                Some(target)
            }
            _ => return None,
        };
        if !labels.contains(&label) || scope.insert(label, target).is_some() {
            return None;
        }
    }
    (scope.len() == labels.len()).then_some(scope)
}

/// The heading the pickup skill writes and the review prompt reads.
///
/// ONE constant for BOTH SIDES OF A HANDSHAKE THAT SPANS A TEMPLATE AND A
/// PARSER. The writer is `aida-core/templates/skills/aida-pickup.md`, the
/// reader is `extract_implementer_approach` below, and nothing in the type
/// system connects them — edit the heading in the template alone and the
/// reader silently stops matching, leaving reviews quietly missing the
/// approach with no error anywhere. The coupling test in
/// `story_1350_review_approach_tests` reads the template and asserts this
/// exact string appears in it, so the two cannot drift apart in one edit.
// trace:STORY-1350 | ai:claude
pub(crate) const IMPLEMENTER_APPROACH_MARKER: &str = "## Implementer approach";

/// Return the newest durable implementation-intent comment. The exact first
/// heading is the stable marker written by the pickup skill; ordinary comments
/// that merely mention an approach must not leak into the review checklist.
// trace:STORY-1350 | ai:codex
pub(crate) fn extract_implementer_approach(
    comments: &[aida_core::models::Comment],
) -> Option<&str> {
    comments.iter().rev().find_map(|comment| {
        let first_line = comment.content.trim_start().lines().next()?;
        (first_line.trim_end() == IMPLEMENTER_APPROACH_MARKER).then_some(comment.content.trim())
    })
}

// trace:STORY-1350 | ai:codex
/// Emit the recorded approach, or SAY THAT THERE IS NONE.
///
/// Silence here is indistinguishable from "the implementer recorded nothing"
/// and from "the section was dropped", and a reviewer reading a prompt with no
/// approach section cannot tell which. Absent evidence has to look different
/// from good evidence, so the absence is stated rather than left as a gap.
// trace:STORY-1350 | ai:claude
pub(crate) fn append_implementer_approach(
    out: &mut String,
    comments: &[aida_core::models::Comment],
) {
    match extract_implementer_approach(comments) {
        Some(approach) => {
            out.push_str("#### Recorded implementer approach\n\n");
            out.push_str(approach);
            out.push_str("\n\n");
        }
        None => {
            out.push_str("#### Recorded implementer approach\n\n");
            // The marker comes from the constant here too. Spelling it out
            // again would make this the THIRD copy — the prose that TELLS a
            // human what to write, drifting from the parser that reads it, so
            // a rename would leave the system parsing X while instructing
            // people to write Y. The coupling test covers this string.
            out.push_str(&format!(
                "_None recorded._ The implementer did not leave a comment beginning \
                 `{IMPLEMENTER_APPROACH_MARKER}`, so this review has no stated intent \
                 to check the diff against.\n\n"
            ));
        }
    }
}

// trace:BUG-800 | ai:codex
pub(crate) fn review_prompt_test_commands_section() -> &'static str {
    "## Test Commands\n\n\
     When you name focused tests, name the runnable command, not a bare test \
     identifier. For Rust, use `cargo test -p <crate> <test_name>` (or the \
     exact workspace/package command you ran) so the next agent does not \
     pass the test name to the wrong binary.\n\n"
}

#[cfg(test)]
mod slice_scope_tests {
    use super::*;

    fn worksheet(messages: &[&str], title: &str, body: &str) -> String {
        let mut req = aida_core::Requirement::new(
            "Slice fixture".to_string(),
            "## Acceptance\n\n- [ ] AC1: [evidence: behavioral-test] First slice\n- [ ] AC2: Next slice".to_string(),
        );
        req.spec_id = Some("BUG-2683".to_string());
        let mut store = aida_core::RequirementsStore::default();
        store.requirements.push(req);
        let coverage = prompt_coverage(
            &messages.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            title,
        );
        let mut out = String::new();
        assert_eq!(
            append_spec_checklist(&mut out, &store, &coverage, body),
            (1, 0)
        );
        out
    }

    const DECLARATION: &str =
        "Slice-Scope: BUG-2683 ac1 covered\r\nSlice-Scope: BUG-2683 AC2 deferred slice-2";

    // trace:BUG-b6b1278890774ef993d2ebc327862470-447.ac2448c8 | ai:codex
    // trace:BUG-b6b1278890774ef993d2ebc327862470-447.ac2a9f9d | ai:codex
    // trace:BUG-b6b1278890774ef993d2ebc327862470-447.Tests | ai:codex
    #[test]
    fn scoped_refs_worksheet_premarks_deferrals() {
        let out = worksheet(
            &["Support plumbing"],
            "First slice (Refs BUG-2683)",
            DECLARATION,
        );
        assert!(out.contains("#### Declared slice scope"));
        assert!(out
            .contains("AC1: [evidence: behavioral-test] First slice — covered — verify evidence"));
        assert!(out.contains("AC2: Next slice — not-applicable-because deferred to `slice-2`"));
    }

    // trace:BUG-b6b1278890774ef993d2ebc327862470-447.ac6cf33f | ai:codex
    // trace:BUG-b6b1278890774ef993d2ebc327862470-447.Tests | ai:codex
    #[test]
    fn unscoped_refs_worksheet_keeps_uncovered_criteria_informational() {
        for body in [
            "",
            "Slice-Scope: BUG-2683 AC1 covered",
            "Slice-Scope: BUG-2683 AC1 covered\nSlice-Scope: BUG-2683 AC1 deferred slice-2\nSlice-Scope: BUG-2683 AC2 covered",
            "Slice-Scope: BUG-2683 AC1 covered\nSlice-Scope: BUG-2683 AC2 deferred `fake`",
        ] {
            let out = worksheet(&["First slice (Refs BUG-2683)"], "", body);
            assert!(out.contains("Slice scope undeclared"));
            assert!(out.contains("List uncovered criteria as informational, not blocking"));
            assert!(out.contains("- [ ] AC2: Next slice"));
            assert!(!out.contains("not-applicable-because deferred"));
        }
    }

    // trace:BUG-b6b1278890774ef993d2ebc327862470-447.ac775474 | ai:codex
    // trace:BUG-b6b1278890774ef993d2ebc327862470-447.Tests | ai:codex
    #[test]
    fn completing_worksheet_keeps_whole_spec_contract() {
        for (message, title) in [
            ("First slice (Refs BUG-2683)", "Complete (BUG-2683)"),
            ("Complete (BUG-2683)", "First slice (Refs BUG-2683)"),
        ] {
            let out = worksheet(&[message], title, DECLARATION);
            assert!(out.contains("- [ ] AC1:"));
            assert!(out.contains("- [ ] AC2:"));
            assert!(!out.contains("slice scope"));
        }
    }
}
