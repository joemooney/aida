// trace:TASK-3428 | ai:codex
use crate::{
    detect_forge_from_origin, extract_acceptance_section, extract_spec_ids_from_commit,
    find_project_root, git_log_messages, parse_review_scope, pr_base_head, stale_base_note,
    ReviewForge, Storage,
};
use anyhow::{Context, Result};
use colored::Colorize;

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
    let (spec_ids, header_subtitle, staleness_note): (Vec<String>, String, Option<String>) =
        if let Some(csv) = specs_csv {
            let ids: Vec<String> = csv
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if ids.is_empty() {
                anyhow::bail!("--specs was empty after splitting on commas");
            }
            (ids.clone(), format!("Specs: {}", ids.join(", ")), None)
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
            let mut ids: Vec<String> = Vec::new();
            for msg in &messages {
                for id in extract_spec_ids_from_commit(msg) {
                    if !ids
                        .iter()
                        .any(|existing| existing.eq_ignore_ascii_case(&id))
                    {
                        ids.push(id);
                    }
                }
            }
            if ids.is_empty() {
                anyhow::bail!(
                    "no `(REQ-ID)` trailers found in {}..{} ({} commits inspected)",
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
            (ids, label, note)
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
    out.push_str("## What to verify\n\n");

    let mut missing: Vec<String> = Vec::new();
    for id in &spec_ids {
        // TASK-72 polish: PR/MR pseudo-IDs (e.g., `PR-9`, `MR-42`) aren't
        // real specs in the store — they're forge references that can
        // sneak into the trailer-extracted list. Skip them silently so
        // the review prompt doesn't lead with a misleading
        // "PR-9 — (not found in store)" row before the real spec list.
        // trace:TASK-72 | ai:claude
        if parse_review_scope(id).is_some() {
            continue;
        }
        let req = store
            .requirements
            .iter()
            .find(|r| r.spec_id.as_deref() == Some(id.as_str()))
            .or_else(|| {
                uuid::Uuid::parse_str(id)
                    .ok()
                    .and_then(|u| store.requirements.iter().find(|r| r.id == u))
            });
        let Some(req) = req else {
            out.push_str(&format!("### {}\n\n_(not found in store)_\n\n", id));
            missing.push(id.clone());
            continue;
        };
        out.push_str(&format!(
            "### {} — {}\n\n",
            req.spec_id.as_deref().unwrap_or(id.as_str()),
            req.title
        ));
        match extract_acceptance_section(&req.description) {
            Some(body) => {
                out.push_str(&body);
                out.push_str("\n\n");
            }
            None => {
                out.push_str("_(no `## Acceptance` / `## Verify` section in description — review against the requirement title and description.)_\n\n");
            }
        }
        append_implementer_approach(&mut out, &req.comments);
    }

    out.push_str(review_prompt_test_commands_section());

    out.push_str("## Decide\n\n");
    out.push_str(
        "If every item above passes: approve and merge (`gh pr merge --squash` / \
         `glab mr merge --squash`), then mark each linked req `completed`.\n\n",
    );
    out.push_str(
        "If any item fails: request changes with specifics tied to the spec_id, \
         so the contributor can address them by id (not by paraphrase).\n",
    );

    if let Some(path) = write_path {
        std::fs::write(path, &out)
            .with_context(|| format!("failed to write review prompt to {}", path))?;
        eprintln!(
            "{} review prompt written to {} ({} spec{}{})",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
            path,
            spec_ids.len(),
            if spec_ids.len() == 1 { "" } else { "s" },
            if missing.is_empty() {
                String::new()
            } else {
                format!(", {} missing in store", missing.len())
            },
        );
    } else {
        print!("{}", out);
    }
    Ok(())
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
