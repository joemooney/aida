// trace:TASK-3781 | ai:codex

use super::*;

/// `aida doctor --fix id-collisions`: renumber spec IDs that differ only by
/// leading zeros (see `doctor_renumber`).
// trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
pub(super) fn doctor_fix_id_collisions(opts: &crate::doctor_renumber::FixOptions) -> Result<()> {
    let project_root = find_project_root()?;
    let store_root = detect_distributed_store_from(&project_root)
        .unwrap_or_else(|| project_root.join(".aida-store"));
    crate::doctor_renumber::run(&project_root, &store_root, opts)
}

/// `aida doctor --fix agreed-ids`: assign agreed IDs to node-local specs that
/// lack one (see `doctor_backfill_agreed`).
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
pub(super) fn doctor_fix_agreed_ids(
    opts: &crate::doctor_backfill_agreed::BackfillOptions,
) -> Result<()> {
    let project_root = find_project_root()?;
    let store_root = detect_distributed_store_from(&project_root)
        .unwrap_or_else(|| project_root.join(".aida-store"));
    crate::doctor_backfill_agreed::run(&store_root, opts)
}

// The rules intentionally match only anomalies that cannot be explained by a
// wrapped prose line. A broad "two spaces" rule produces table/code noise and
// cannot distinguish a typo from lost command-substitution text.
// trace:TASK-190 | ai:codex
pub(super) fn shell_substitution_hole_kind(line: &str) -> Option<&'static str> {
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

pub(super) fn doctor_shell_substitution_holes(exclude: &[String]) -> Result<()> {
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

pub(super) fn doctor_bare_id_twins() -> Result<()> {
    let project_root = find_project_root()?;
    let objects_root = shell_substitution_objects_root(&project_root);
    if !objects_root.exists() {
        println!("(no objects/ tree — nothing to check)");
        return Ok(());
    }
    let reqs = aida_core::object_store::load_all_objects(&objects_root)?;
    let lines = ambiguous_bare_id_lines(&reqs);
    for line in &lines {
        println!("{line}");
    }
    println!(
        "{} ambiguous bare id(s); write the local spec in its alias form (this check never writes)",
        lines.len()
    );
    Ok(())
}

/// One report line per bare id in a description or comment that has a
/// node-local twin: `SPEC:field:line: BARE also names local ALIAS (...)`. The
/// local ids are indexed once by `(TYPE, number)`, so the scan is linear in
/// the prose. A twin is named by its agreed id when it has one, and a bare id
/// that IS that twin's agreed id is not ambiguous. Ids are compared by their
/// `(TYPE, number)` key, so `BUG-030` and a stored `BUG-30` are the same id.
/// Pure over `reqs` so a fixture can drive it.
// trace:BUG-B6B1278890774EF993D2EBC327862470-036 | ai:claude
pub(crate) fn ambiguous_bare_id_lines(reqs: &[aida_core::Requirement]) -> Vec<String> {
    use aida_core::node_alias::{
        bare_id_tokens, display_spec_id, local_id_key, local_twin_label, split_bare_id,
    };
    // (TYPE, number) -> (label, agreed-id key) per local spec.
    type Key = (String, u32);
    let mut locals: std::collections::HashMap<Key, Vec<(String, Option<Key>)>> = Default::default();
    let mut agreed: std::collections::HashSet<Key> = Default::default();
    for req in reqs {
        for id in req.spec_id.iter().chain(req.agreed_id.iter()) {
            match local_id_key(id) {
                Some(key) => locals.entry(key).or_default().push((
                    local_twin_label(id, req.agreed_id.as_deref()),
                    req.agreed_id.as_deref().and_then(split_bare_id),
                )),
                None => agreed.extend(split_bare_id(id)),
            }
        }
    }
    let mut out = Vec::new();
    for req in reqs {
        let id = display_spec_id(req.spec_id.as_deref().unwrap_or("<unknown>"));
        let fields = std::iter::once(("description", req.description.as_str())).chain(
            req.comments
                .iter()
                .map(|comment| ("comment", comment.content.as_str())),
        );
        for (field, body) in fields {
            for (line, token) in bare_id_tokens(body) {
                let Some(key) = split_bare_id(&token) else {
                    continue;
                };
                let Some(twins) = locals.get(&key) else {
                    continue;
                };
                let twins: Vec<&str> = twins
                    .iter()
                    .filter(|(_, twin_agreed)| twin_agreed.as_ref() != Some(&key))
                    .map(|(label, _)| label.as_str())
                    .collect();
                if twins.is_empty() {
                    continue;
                }
                let resolves = if agreed.contains(&key) {
                    format!("bare {token} opens the agreed spec")
                } else {
                    format!("bare {token} is not found")
                };
                out.push(format!(
                    "{id}:{field}:{line}: {token} also names local {} ({resolves})",
                    twins.join(", ")
                ));
            }
        }
    }
    out
}

pub(super) fn shell_substitution_objects_root(
    project_root: &std::path::Path,
) -> std::path::PathBuf {
    detect_distributed_store_from(project_root)
        .unwrap_or_else(|| project_root.join(".aida-store"))
        .join("objects")
}

// trace:BUG-2490 | ai:codex
pub(super) fn scan_store_objects(store_path: &std::path::Path) -> Vec<DoctorFinding> {
    aida_core::git_ops::store_object_findings(store_path).into_iter().map(|(path, error)| DoctorFinding {
        category: "store-objects".to_string(),
        id: path.display().to_string(),
        summary: error,
        action: format!("resolve conflict markers or restore valid requirement YAML in {}; then rerun doctor", path.display()),
        safe_heal: false,
    }).collect()
}

/// BUG-1505: surface every review-verdict file that is not in the canonical
/// shape. An UNKNOWN / ambiguous verdict word (e.g. a qualified approval such
/// as `APPROVED pending cross-platform green`) gets its OWN finding: every
/// gate reads it as not-approved, and only a human can say what it meant.
/// Everything else (a re-spelling, a legacy key, a missing sha) is folded into
/// one aggregate finding so a large legacy corpus does not drown the report.
// trace:BUG-1505 | ai:claude
pub(super) fn scan_review_verdicts(project_root: &std::path::Path) -> Vec<DoctorFinding> {
    let rows = crate::review_verdict_display::audit_verdict_dir(project_root);
    let mut out = Vec::new();
    let mut drifted = Vec::new();
    for row in &rows {
        if row.kind == crate::gate::review_verdict::VerdictKind::Unknown && !row.raw.is_empty() {
            out.push(DoctorFinding {
                category: "review-verdicts".to_string(),
                id: format!("unknown-verdict:{}", row.file),
                summary: format!(
                    "{} records verdict `{}`, which is not a recognised verdict — every gate reads it as NOT approved",
                    row.file, row.raw
                ),
                action: format!(
                    "decide it by hand, then record a fresh verdict: `aida review record <SPEC> --verdict approved|request-changes|rejected|waiting --summary \"<why>\"` (file: .aida/review-verdicts/{})",
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
pub(super) fn scan_store_scrub(project_root: &std::path::Path) -> Vec<DoctorFinding> {
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
    // raw-git-ok(BUG-1929): read-only `log` in the code checkout, naming the ref.
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
                "landed before redaction was enabled; scrub it from the store history before it propagates (see docs/internal/security/)"
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
pub(super) fn round_trip_toon_header_regex() -> regex::Regex {
    // Anchored at column 0 of a line (`(?m)^`). Anchoring is what separates a
    // genuine swallowed TOON rendering — which always starts at a line's
    // first column, because that is where the renderer emits it — from prose
    // that merely discusses the convention mid-line. Unanchored, this very
    // check's own spec text (which quotes the pattern) would false-positive.
    regex::Regex::new(r"(?m)^(?:(?:relationships|next)\[\d+\]\{|execution_mode:)")
        .expect("valid TOON round-trip header regex")
}

pub(super) fn round_trip_toon_header_regex_unanchored() -> regex::Regex {
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

pub(super) fn round_trip_mojibake_regex() -> regex::Regex {
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
pub(super) fn push_comment_fields<'a>(
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
pub(super) fn is_indent_anchored(bytes: &[u8], start: usize) -> bool {
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
pub(super) fn scan_round_trip_artifacts(
    store: &aida_core::models::RequirementsStore,
) -> Vec<DoctorFinding> {
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

// Relationship edges are the source of truth; rebuild only the denormalized
// parent:* tags for this requirement. trace:BUG-1252 | ai:codex
pub(crate) fn heal_doctor_parent_tag_drift(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let store_path = project_root.join(".aida-store");
    let storage = Storage::new(&store_path);
    // Read, validate and save under the existing writer lock. The save's inner
    // acquisition is reentrant on this thread. trace:TASK-1981 | ai:codex
    aida_core::db::with_store_write_lock(&store_path, || -> Result<()> {
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
        Ok(())
    })?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: finding.action.clone(),
        status: "healed".to_string(),
        detail: None,
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
pub(super) fn heal_doctor_external_import_bleed(
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

pub(crate) fn heal_doctor_spec_status(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    // BUG-1944: the snapshot is read-only. Each write goes through the
    // per-spec atomic update, which re-reads the spec under the store lock and
    // re-checks its status there, so the live-work probes (which may wait on
    // git and the forge) never hold a stale whole-store copy that a save would
    // write back over concurrent changes.
    // trace:BUG-1944 | ai:claude
    let storage = Storage::new(project_root.join(".aida-store"));
    let store_root = project_root.join(".aida-store");
    let matching: Vec<aida_core::Requirement> = storage
        .load()?
        .requirements
        .into_iter()
        .filter(|req| req.spec_id.as_deref() == Some(finding.id.as_str()))
        .collect();
    let scope_lease_held = || {
        list_leases(project_root)
            .iter()
            .any(|lease| lease.scope.eq_ignore_ascii_case(&finding.id))
    };
    let current_scope_lease = scope_lease_held();
    let mut action = None;
    let mut kept: Option<String> = None;
    for req in &matching {
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
            crate::status_write_race_seam(&store_root);
            let mut applied = false;
            storage.update_spec_atomically(req, |r| {
                if matches!(r.status, RequirementStatus::Approved) {
                    // BUG-1637: an automated repair, recorded under the
                    // doctor's author so the BUG-1625 merge guard can see it.
                    // trace:BUG-1637 | ai:claude
                    aida_core::conflict::set_status_recorded(
                        r,
                        RequirementStatus::InProgress,
                        aida_core::conflict::DOCTOR_AUTHOR,
                    );
                    r.modified_at = chrono::Utc::now();
                    applied = true;
                }
            })?;
            if applied {
                action = Some("bumped Approved spec to In Progress".to_string());
            }
        } else if matches!(req.status, RequirementStatus::InProgress)
            && finding.action.contains("no active lease")
        {
            if current_scope_lease {
                continue;
            }
            // BUG-1944: a missing lease is not abandoned work. While a branch
            // carrying the spec has an open PR or unmerged commits, or the
            // forge can't say, the spec stays In Progress.
            // trace:BUG-1944 | ai:claude
            if let crate::live_work::LiveWorkVerdict::Keep(why) =
                crate::live_work::spec_live_work(project_root, &finding.id)
            {
                kept = Some(why);
                continue;
            }
            // The probes take time; a session may have taken the spec since.
            if scope_lease_held() {
                continue;
            }
            crate::status_write_race_seam(&store_root);
            let mut applied = false;
            storage.update_spec_atomically(req, |r| {
                if matches!(r.status, RequirementStatus::InProgress) {
                    // trace:BUG-1637 | ai:claude
                    aida_core::conflict::set_status_recorded(
                        r,
                        RequirementStatus::Approved,
                        aida_core::conflict::DOCTOR_AUTHOR,
                    );
                    r.modified_at = chrono::Utc::now();
                    applied = true;
                }
            })?;
            if applied {
                action = Some(format!(
                    "reverted {} from In Progress to Approved (no active lease found)",
                    finding.id
                ));
            }
        }
    }
    let changed = action.is_some();
    // trace:BUG-1944 | ai:claude
    let default_action = match kept {
        Some(why) => format!("kept {} In Progress: {why}", finding.id),
        None => "no automatic status change applied".to_string(),
    };
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: action.unwrap_or(default_action),
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
pub(super) fn heal_doctor_completed_without_commit(
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

/// TASK-752: heal a tracked legacy-store-cruft finding — exactly PR-651's
/// resolution: `git rm` the file from the tree, then append the gitignore block
/// (`requirements*.yaml`, `scaffold-report.html`) so the artifacts can't return.
/// Idempotent: a path already untracked (e.g. a prior heal removed it) reports
// `skipped`; the gitignore block is appended only once. trace:TASK-752 | ai:claude
pub(super) fn heal_doctor_legacy_store_cruft(
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
pub(super) fn ensure_legacy_store_cruft_gitignore(project_root: &std::path::Path) -> Result<bool> {
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
pub(super) fn heal_doctor_store_tracked_runtime(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let store_worktree = project_root.join(".aida-store");
    let path = &finding.id;

    // trace:TASK-1981 | ai:agy
    aida_core::db::with_store_write_lock(&store_worktree, || {
        // Re-confirm the file is still tracked on the orphan branch before removing —
        // between scan and heal another heal (or a manual `git rm`) may have already
        // dropped it.
        let still_tracked = aida_core::git_ops::host_git_command(&store_worktree)
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
        let status = aida_core::git_ops::host_git_command(&store_worktree)
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
            let _ = aida_core::git_ops::host_git_command(&store_worktree)
                .args(["add", ".gitignore"])
                .status();
        }
        let commit = aida_core::git_ops::host_git_command(&store_worktree)
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
    })
}

/// Ensure the per-clone runtime and atomic-write staging guards are present in
/// the STORE worktree's `.gitignore`. Each concern has its own idempotence
/// sentinel; returns whether either guard was appended.
// trace:BUG-563 | ai:claude
// trace:TASK-1556 | ai:codex
pub(super) const STORE_RUNTIME_GITIGNORE_PATTERNS: &[&str] = &[
    ".aida/node.toml",
    ".aida/dispenser.toml",
    ".aida/*.lock",
    ".aida/cache.db",
    ".aida/cache.db-journal",
    ".aida/cache.db-shm",
    ".aida/cache.db-wal",
    // The agreed-id dispense lock beside `registry/blocks.yaml`.
    // trace:BUG-2378 | ai:claude
    "registry/*.lock",
];

pub(crate) fn ensure_store_tracked_runtime_gitignore(
    store_worktree: &std::path::Path,
) -> Result<bool> {
    ensure_store_tracked_runtime_gitignore_with_patterns(
        store_worktree,
        aida_core::fs_atomic::STORE_STAGING_IGNORE_PATTERNS,
    )
}

// trace:TASK-1556 | ai:codex
pub(super) fn ensure_store_tracked_runtime_gitignore_with_patterns(
    store_worktree: &std::path::Path,
    staging_patterns: &[&str],
) -> Result<bool> {
    // trace:TASK-1981 | ai:agy
    aida_core::db::with_store_write_lock(store_worktree, || {
        use std::io::Write;
        let gitignore_path = store_worktree.join(".gitignore");
        let existing_bytes = std::fs::read(&gitignore_path).unwrap_or_default();
        let existing = String::from_utf8_lossy(&existing_bytes);
        let lines: Vec<&str> = existing.lines().map(str::trim).collect();
        let mut blocks = String::new();

        // Check every line in the runtime block so new rules cannot be hidden
        // behind a one-pattern sentinel.
        let missing_runtime_patterns: Vec<_> = STORE_RUNTIME_GITIGNORE_PATTERNS
            .iter()
            .filter(|pattern| !lines.contains(pattern))
            .collect();
        if !missing_runtime_patterns.is_empty() {
            blocks.push_str(
                "\n# Per-clone runtime state — must never be tracked on the orphan aida-store branch\n",
            );
            for pattern in missing_runtime_patterns {
                blocks.push_str(pattern);
                blocks.push('\n');
            }
        }

        // BUG-1677 staging ignores are owned by fs_atomic. Keep them in this
        // tracked store `.gitignore`, never the shared project `info/exclude`, so
        // lock-free `git add -A .` cannot stage a temp file or hide user files.
        // trace:TASK-1547 | ai:codex
        // trace:TASK-1556 | ai:codex
        let missing_staging_patterns: Vec<_> = staging_patterns
            .iter()
            .filter(|pattern| !lines.contains(pattern))
            .collect();
        if !missing_staging_patterns.is_empty() {
            blocks.push_str("\n# Atomic-write staging files (temp+rename) — never tracked\n");
            for pattern in missing_staging_patterns {
                blocks.push_str(pattern);
                blocks.push('\n');
            }
        }
        if blocks.is_empty() {
            return Ok(false);
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore_path)
            .with_context(|| format!("opening {}", gitignore_path.display()))?;
        if !existing_bytes.is_empty() {
            f.write_all(b"\n").with_context(|| {
                format!("separating appended rules in {}", gitignore_path.display())
            })?;
        }
        f.write_all(blocks.trim_start_matches('\n').as_bytes())
            .with_context(|| format!("appending to {}", gitignore_path.display()))?;
        Ok(true)
    })
}

// A git -C on a missing/non-repository store path walks upward. Require the
// exact `.aida-store` directory to be the repository root before any writes.
// trace:TASK-1547 | ai:codex
pub(super) fn is_intended_store_worktree(
    project_root: &std::path::Path,
    store_worktree: &std::path::Path,
) -> bool {
    let Some(canonical_store) =
        aida_core::store_locate::detect_distributed_store_from(project_root)
    else {
        return false;
    };
    let Ok(expected_root) = std::fs::canonicalize(store_worktree) else {
        return false;
    };
    if std::fs::canonicalize(canonical_store).ok().as_ref() != Some(&expected_root) {
        return false;
    }
    let Ok(output) = aida_core::git_ops::host_git_command(store_worktree)
        .args(["rev-parse", "--show-toplevel"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let reported = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !std::fs::canonicalize(reported).is_ok_and(|root| root == expected_root) {
        return false;
    }
    // A standalone repository at `.aida-store` can pass the root check too.
    // An attached linked worktree has a `.git` file and resolves a distinct
    // git-dir and common-dir; require both, plus the canonical orphan branch.
    if !std::fs::symlink_metadata(store_worktree.join(".git"))
        .is_ok_and(|metadata| metadata.file_type().is_file())
    {
        return false;
    }
    let git_value = |args: &[&str]| {
        aida_core::git_ops::host_git_command(store_worktree)
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    let (Some(git_dir), Some(common_dir), Some(branch)) = (
        git_value(&["rev-parse", "--git-dir"]),
        git_value(&["rev-parse", "--git-common-dir"]),
        git_value(&["symbolic-ref", "--quiet", "--short", "HEAD"]),
    ) else {
        return false;
    };
    if git_dir == common_dir || branch != "aida-store" {
        return false;
    }
    let canonical_common_dir = |root: &std::path::Path| {
        let common_dir = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "--git-common-dir"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())?;
        let common_dir = std::path::PathBuf::from(common_dir);
        let common_dir = if common_dir.is_absolute() {
            common_dir
        } else {
            root.join(common_dir)
        };
        std::fs::canonicalize(common_dir).ok()
    };
    matches!(
        (
            canonical_common_dir(store_worktree),
            canonical_common_dir(project_root)
        ),
        (Some(store_common_dir), Some(project_common_dir))
            if store_common_dir == project_common_dir
    )
}

// TASK-1547: repair the independent staging-ignore finding on the orphan
// branch, including stores already healed by BUG-563.
// trace:TASK-1547 | ai:codex
pub(super) fn heal_doctor_store_staging_ignore(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let store_worktree = project_root.join(".aida-store");
    if !is_intended_store_worktree(project_root, &store_worktree) {
        return Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "failed".to_string(),
            detail: Some(
                ".aida-store is not the root of an attached git worktree; left unchanged"
                    .to_string(),
            ),
        });
    }
    // trace:TASK-1981 | ai:agy
    aida_core::db::with_store_write_lock(&store_worktree, || {
        let appended = ensure_store_tracked_runtime_gitignore(&store_worktree)?;
        if appended {
            let add = aida_core::git_ops::host_git_command(&store_worktree)
                .args(["add", ".gitignore"])
                .status()
                .with_context(|| "staging store .gitignore".to_string())?;
            if !add.success() {
                anyhow::bail!("git add .gitignore failed in store worktree");
            }
            let commit = aida_core::git_ops::host_git_command(&store_worktree)
                .args([
                    "commit",
                    "--only",
                    "--quiet",
                    "-m",
                    "chore(store): ignore atomic staging files (TASK-1547 TASK-1556)",
                    "--",
                    ".gitignore",
                ])
                .status()
                .with_context(|| "committing staging ignores on store worktree".to_string())?;
            if !commit.success() {
                return Ok(DoctorHealResult {
                    category: finding.category.clone(),
                    id: finding.id.clone(),
                    action: finding.action.clone(),
                    status: "failed".to_string(),
                    detail: Some("commit on store worktree failed".to_string()),
                });
            }
        }
        Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: "append missing staging patterns and commit on store worktree".to_string(),
            status: if appended { "healed" } else { "skipped" }.to_string(),
            detail: None,
        })
    })
}

// TASK-570: heal an orphan queue entry by routing through the same
// `storage.queue_remove` primitive that `aida queue prune --orphaned`
// (TASK-537) uses. Idempotent — a re-run after the entry is already gone
// reports `skipped`. trace:TASK-570 | ai:claude
pub(super) fn heal_doctor_orphan_queue_entry(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
) -> Result<DoctorHealResult> {
    let user_id = current_user_id(None);
    let store_path = project_root.join(".aida-store");
    let storage = Storage::new(&store_path);
    let entry_uuid: uuid::Uuid = finding
        .id
        .parse()
        .with_context(|| format!("parsing queue entry id {}", finding.id))?;
    // trace:TASK-1981 | ai:agy
    aida_core::db::with_store_write_lock(&store_path, || {
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
    })
}

/// STORY-70: walk the orphan store, flag STORY/BUG requirements whose
/// descriptions don't contain a recognized acceptance heading. Output
/// shape mirrors the other doctor commands (per-finding rows + a final
/// summary). Exits non-zero on findings so CI/scripts can gate on it.
// trace:STORY-70 | ai:claude
pub(super) fn doctor_convention_check(quiet: bool) -> Result<()> {
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
pub(super) fn doctor_verify_relationships(repair: bool, yes: bool) -> Result<()> {
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
        rel_type: aida_core::RelationshipType,
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
                    rel_type: rel.rel_type.clone(),
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
            format!("{:?}", d.rel_type).dimmed(),
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

    // Repair: re-load canonical objects inside the store write lock so any
    // concurrent spec edit made while waiting for confirmation (or the lock) is
    // preserved rather than overwritten by the pre-prompt `reqs` snapshot.
    // When confirmed interactively (`!yes`), only strip edges that were in the
    // user-confirmed preview set AND are still dangling under the lock.
    // trace:TASK-58 trace:TASK-1930 trace:TASK-1981 | ai:agy
    let confirmed_edges: Option<
        std::collections::HashSet<(uuid::Uuid, uuid::Uuid, aida_core::RelationshipType)>,
    > = (!yes).then(|| {
        dangling
            .iter()
            .map(|d| (d.source_uuid, d.target_uuid, d.rel_type.clone()))
            .collect()
    });
    let _ = reqs;
    let fixed =
        doctor_repair_dangling_relationships_in_store(&store_path, confirmed_edges.as_ref())?;
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
pub(super) fn doctor_validate_trace_comments(
    strip_dangling: bool,
    dry_run: bool,
    yes: bool,
) -> Result<()> {
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
    // comments, while agreed_id is the canonical post-merge form. Keyed
    // upper-case: node-local ids carry a hex node segment written in either
    // case. trace:TASK-b6b1278890774ef993d2ebc327862470-007 | ai:claude
    // trace:EPIC-19 | ai:claude
    let mut yaml_files: Vec<std::path::PathBuf> = Vec::new();
    walk_yamls(&objects_root, &mut yaml_files);
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude — numeric identity keys.
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
                    known_specs.insert(aida_core::id_identity::id_key(&s));
                }
            } else if let Some(v) = t.strip_prefix("agreed_id:") {
                let s = v.trim().trim_matches('"').trim_matches('\'').to_string();
                // agreed_id can be `null` / empty in YAML; skip those.
                if !s.is_empty() && s != "null" && s != "~" {
                    known_specs.insert(aida_core::id_identity::id_key(&s));
                }
            }
        }
    }

    // Collect every trace comment in the project tree.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-007 | ai:claude
    let trace_re = crate::trace_marker_re().clone();
    let mut by_spec: std::collections::HashMap<String, Vec<(std::path::PathBuf, usize)>> =
        std::collections::HashMap::new();

    walk_source_for_traces(&project_root, &trace_re, &mut by_spec);

    let mut orphan_specs: Vec<&String> = by_spec
        .keys()
        .filter(|s| !known_specs.contains(&aida_core::id_identity::id_key(s)))
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
pub(super) struct StripStats {
    pub(super) files_changed: usize,
    pub(super) lines_deleted: usize,
    pub(super) lines_modified: usize,
}

/// Walk every text source file under `root` and either delete or modify
// any line containing `trace:<DANGLING>` per the dangling_ids set.
// When `dry_run`, returns counts without writing. trace:EPIC-19
pub(super) fn strip_dangling_traces(
    root: &std::path::Path,
    dangling_ids: &std::collections::HashSet<String>,
    dry_run: bool,
) -> Result<StripStats> {
    let mut stats = StripStats::default();
    strip_dangling_walk(root, dangling_ids, dry_run, &mut stats);
    Ok(stats)
}

pub(super) fn strip_dangling_walk(
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
pub(super) fn rewrite_strip_dangling(
    content: &str,
    dangling_ids: &std::collections::HashSet<String>,
) -> (String, usize, usize) {
    use regex::Regex;
    // Match `trace:<ID> | ai:<tool>(:<conf>)?` fragments. Capture the
    // id so we can check it against the dangling set.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-007 | ai:claude
    let frag_re = Regex::new(&format!(
        r"trace:({})\s*\|\s*ai:[a-zA-Z]+(?::(?:high|med|low))?",
        crate::CANONICAL_SPEC_ID_PATTERN
    ))
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
pub(super) fn walk_yamls(root: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
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
pub(super) fn doctor_repair_stale_blocks(dry_run: bool, yes: bool) -> Result<()> {
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

    // trace:TASK-1930 trace:TASK-1981 | ai:agy — A1 lock order: store_write_lock -> blocks.lock,
    // and re-load blocks.yaml + nodes.toml inside the lock so concurrent `next`
    // advances or node registrations since the preview are preserved. When
    // confirmed interactively (`!yes`), only tombstone blocks in the confirmed set.
    let confirmed_blocks: Option<std::collections::HashSet<(String, String, u32, u32)>> = (!yes)
        .then(|| {
            stale
                .iter()
                .map(|(_, b)| {
                    (
                        b.node_id.clone(),
                        b.type_prefix.clone(),
                        b.range_start,
                        b.range_end,
                    )
                })
                .collect()
        });
    let _ = blocks;
    let count = doctor_tombstone_stale_blocks_in_store(&store_path, confirmed_blocks.as_ref())?;

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
pub(super) fn doctor_scrub_collisions() -> Result<()> {
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
        // trace:BUG-1923 | ai:claude
        let by = st
            .lag
            .map(|n| format!(" by {n} commit(s)"))
            .unwrap_or_default();
        if st.head_behind_tip || st.tip_live_elsewhere {
            // trace:BUG-1998 | ai:antigravity
            // trace:BUG-1998 | ai:claude
            // Before the stall flag: behind or elsewhere gets no rebuild advice.
            return format!(
                "history index behind the store{by}{running} — another checkout moved \
                 the shared index on."
            );
        }
        if st.catch_up_stalled {
            return format!(
                "history index behind the store{by}{running} and not catching up — \
                 run `aida cache rebuild --history`."
            );
        }
        return format!(
            // trace:STORY-1650 | ai:claude
            "history index behind the store{by}{running} — it catches up on the next \
             `aida history events` query."
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
pub(super) fn doctor_fsck() -> Result<()> {
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
        // trace:TASK-b6b1278890774ef993d2ebc327862470-007 | ai:claude
        let trace_re = crate::trace_marker_re().clone();
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
                            known_specs.insert(aida_core::id_identity::id_key(&s));
                        }
                    } else if let Some(v) = t.strip_prefix("agreed_id:") {
                        let s = v.trim().trim_matches('"').trim_matches('\'').to_string();
                        if !s.is_empty() && s != "null" && s != "~" {
                            known_specs.insert(aida_core::id_identity::id_key(&s));
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
            .filter(|(s, _)| !known_specs.contains(&aida_core::id_identity::id_key(s)))
            .map(|(_, v)| v.len())
            .sum();
        let dangling_specs: usize = by_spec
            .keys()
            .filter(|s| !known_specs.contains(&aida_core::id_identity::id_key(s)))
            .count();
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

pub(super) fn doctor_migrate_counter_scope(
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

    // Apply: acquire store_write_lock -> blocks.lock (A1), re-load blocks.yaml
    // and re-validate inside the lock so concurrent dispenses or block claims
    // since the preview are preserved. When confirmed interactively (`!yes`),
    // only retire blocks that were in the user-confirmed preview set.
    // trace:TASK-1930 trace:TASK-1981 | ai:agy
    let confirmed_blocks: Option<std::collections::HashSet<(String, u32, u32)>> =
        (!yes).then(|| {
            our_blocks
                .iter()
                .map(|b| (b.type_prefix.clone(), b.range_start, b.range_end))
                .collect()
        });
    let _ = registry;
    let (new_start, new_end) = doctor_migrate_counter_scope_in_store(
        &project_root,
        &store_path,
        &node_id,
        new_block_size,
        confirmed_blocks.as_ref(),
    )?;

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
