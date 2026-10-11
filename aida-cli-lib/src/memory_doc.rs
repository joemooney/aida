//! Scaffolded memory-pack refresh, drift reporting and markdown frontmatter helpers (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// FNV-1a 64-bit hash, lowercase hex. Used as the starter memory pack's
/// "edited since scaffold?" fingerprint — deterministic across releases and
/// platforms, no dependency. trace:STORY-255 | ai:claude
pub(crate) fn fnv1a_hex(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Normalize Windows (`\r\n`) and classic-Mac (`\r`) line endings to `\n`
/// so the frontmatter parser is line-ending-agnostic. Git on Windows with
/// `autocrlf` checks the embedded memory templates out as CRLF, which
/// `build.rs` then embeds verbatim — without this the `---\n` frontmatter
/// fence never matches and the template is reported malformed.
/// trace:BUG-244 | ai:claude
pub(crate) fn normalize_line_endings(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Split a markdown file into (frontmatter, body) at the leading `---`
/// fenced YAML block. Returns `None` when there is no frontmatter. Callers
/// pass LF-normalized input (see `normalize_line_endings`).
pub(crate) fn split_md_frontmatter(content: &str) -> Option<(&str, &str)> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some((&rest[..end], &rest[end + 5..]))
}

/// Read a top-level scalar frontmatter field. Indented (nested) keys are
/// ignored on purpose — `originSessionId` must be a top-level key for the
/// refresh check to see it. trace:STORY-255 | ai:claude
pub(crate) fn frontmatter_field<'a>(frontmatter: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}: ");
    frontmatter
        .lines()
        .find_map(|l| l.strip_prefix(prefix.as_str()))
        .map(str::trim)
}

/// Build the on-disk form of a starter memory from its embedded template:
/// stamp `originSessionId: aida-scaffold` and a `scaffoldChecksum` (an
/// FNV-1a fingerprint of the body) into the frontmatter, so a later
/// `--refresh` can tell a pristine pack file from a user-edited one.
/// trace:STORY-255 | ai:claude
pub(crate) fn build_scaffolded_memory(template: &str) -> Option<String> {
    let template = normalize_line_endings(template);
    let (fm, body) = split_md_frontmatter(&template)?;
    let body = body.trim_end();
    let checksum = fnv1a_hex(body.as_bytes());
    let mut new_fm = String::new();
    for line in fm.lines() {
        if line.starts_with("originSessionId:") || line.starts_with("scaffoldChecksum:") {
            continue;
        }
        new_fm.push_str(line);
        new_fm.push('\n');
    }
    new_fm.push_str("originSessionId: aida-scaffold\n");
    new_fm.push_str(&format!("scaffoldChecksum: {checksum}\n"));
    Some(format!("---\n{new_fm}---\n{body}\n"))
}

/// What `aida init --with-memories --refresh` should do with an existing
/// memory file. trace:STORY-255 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MemoryDisposition {
    /// Not a scaffolded pack file (no `originSessionId: aida-scaffold`) —
    /// the user wrote it; never touch it.
    UserOwned,
    /// Scaffolded, but the body no longer matches its recorded checksum —
    /// the user has edited it; keep their edits.
    Edited,
    /// Scaffolded and untouched since — safe to overlay a new version.
    Pristine,
}

/// Classify an existing memory file for `--refresh`.
pub(crate) fn memory_refresh_disposition(existing: &str) -> MemoryDisposition {
    let existing = normalize_line_endings(existing);
    let Some((fm, body)) = split_md_frontmatter(&existing) else {
        return MemoryDisposition::UserOwned;
    };
    if frontmatter_field(fm, "originSessionId") != Some("aida-scaffold") {
        return MemoryDisposition::UserOwned;
    }
    match frontmatter_field(fm, "scaffoldChecksum") {
        Some(stored) if stored == fnv1a_hex(body.trim_end().as_bytes()) => {
            MemoryDisposition::Pristine
        }
        _ => MemoryDisposition::Edited,
    }
}

/// Regenerate the `aida:scaffold-pack` block in the memory dir's MEMORY.md
/// index. Content outside the markers is the user's and is preserved.
/// trace:STORY-255 | ai:claude
pub(crate) fn update_memory_index(mem_dir: &std::path::Path, files: &[(&str, &str)]) -> Result<()> {
    const START: &str = "<!-- aida:scaffold-pack:start -->";
    const END: &str = "<!-- aida:scaffold-pack:end -->";

    let mut entries = String::new();
    for (filename, template) in files {
        let template = normalize_line_endings(template);
        let (fm, _) = split_md_frontmatter(&template).unwrap_or(("", ""));
        let label = frontmatter_field(fm, "name").unwrap_or(filename);
        let desc = frontmatter_field(fm, "description").unwrap_or("");
        entries.push_str(&format!("- [{label}]({filename}) — {desc}\n"));
    }
    let block = format!("{START}\n{entries}{END}");

    let index_path = mem_dir.join("MEMORY.md");
    let content = if index_path.exists() {
        let existing = std::fs::read_to_string(&index_path)?;
        match (existing.find(START), existing.find(END)) {
            (Some(s), Some(e)) if e > s => {
                format!("{}{}{}", &existing[..s], block, &existing[e + END.len()..])
            }
            _ => {
                let sep = if existing.ends_with('\n') {
                    "\n"
                } else {
                    "\n\n"
                };
                format!("{existing}{sep}{block}\n")
            }
        }
    } else {
        let skeleton = aida_core::templates::EMBEDDED_TEMPLATES
            .get("memories/MEMORY.md")
            .copied()
            .unwrap_or("# Project Memory Index");
        match (skeleton.find(START), skeleton.find(END)) {
            (Some(s), Some(e)) if e > s => {
                format!(
                    "{}{}{}\n",
                    &skeleton[..s],
                    block,
                    &skeleton[e + END.len()..]
                )
            }
            _ => format!("{skeleton}\n\n{block}\n"),
        }
    };
    std::fs::write(&index_path, content)?;
    Ok(())
}

/// Per-disposition counts from a starter-memory-pack scaffold/refresh run.
/// trace:STORY-255 | ai:claude
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct MemoryPackReport {
    /// Files that did not exist and were freshly written.
    pub(crate) written: usize,
    /// Pristine pack files overlaid with a newer version (`--refresh`).
    pub(crate) refreshed: usize,
    /// Files left as-is — already current, or `--with-memories` without
    /// `--refresh` finding an existing file.
    pub(crate) unchanged: usize,
    /// Scaffolded files the user has edited — kept (`--refresh`).
    pub(crate) kept_edited: usize,
    /// Files with no `aida-scaffold` marker — the user's own; kept.
    pub(crate) kept_user: usize,
}

/// Decide whether a memory template loads under an active `--focus`
/// subsystem. A memory with no top-level `subsystem:` frontmatter key is
/// **universal** and always loads. A tagged memory loads only when its
/// `subsystem:` value matches `focus` (case-insensitive). When `focus` is
/// `None` the full pack loads (every member passes). trace:STORY-362 | ai:claude
pub(crate) fn memory_matches_focus(template: &str, focus: Option<&str>) -> bool {
    let Some(focus) = focus else {
        return true; // no focus → full pack
    };
    let template = normalize_line_endings(template);
    let Some((fm, _)) = split_md_frontmatter(&template) else {
        return true; // malformed/no frontmatter → treat as universal
    };
    match frontmatter_field(fm, "subsystem") {
        Some(subsystem) => subsystem.eq_ignore_ascii_case(focus),
        None => true, // untagged → universal, always loads
    }
}

/// Write (or `--refresh`) the starter memory pack into `mem_dir`. Every
/// embedded `memories/*` template except the MEMORY.md skeleton is a pack
/// member; MEMORY.md's `aida:scaffold-pack` index block is regenerated.
/// The pure core of `scaffold_memory_pack` — takes the target dir directly
/// so it is testable without touching the real `$HOME`.
///
/// `focus` scopes the pack to a subsystem (STORY-362): when `Some`, only
/// universal (untagged) memories plus those whose `subsystem:` frontmatter
/// matches are written/indexed; `None` writes the full pack.
/// trace:STORY-255 | ai:claude
pub(crate) fn scaffold_memory_pack_into(
    mem_dir: &std::path::Path,
    refresh: bool,
    focus: Option<&str>,
) -> Result<MemoryPackReport> {
    use aida_core::templates::EMBEDDED_TEMPLATES;

    let all_members = EMBEDDED_TEMPLATES.iter().any(|(k, _)| {
        k.strip_prefix("memories/")
            .is_some_and(|n| n != "MEMORY.md")
    });
    if !all_members {
        anyhow::bail!("no starter memories are embedded in this build");
    }

    // trace:STORY-362 | ai:claude — apply the --focus subsystem filter:
    // untagged memories are universal and always pass.
    let mut files: Vec<(&str, &str)> = EMBEDDED_TEMPLATES
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("memories/")
                .filter(|n| *n != "MEMORY.md")
                .map(|n| (n, *v))
        })
        .filter(|(_, template)| memory_matches_focus(template, focus))
        .collect();
    if files.is_empty() {
        anyhow::bail!(
            "no starter memories match the requested focus subsystem (and none are universal)"
        );
    }
    files.sort_by(|a, b| a.0.cmp(b.0));

    std::fs::create_dir_all(mem_dir)?;
    let mut report = MemoryPackReport::default();

    for (name, template) in &files {
        let dest = mem_dir.join(name);
        let scaffolded = build_scaffolded_memory(template)
            .with_context(|| format!("malformed starter memory template: {name}"))?;

        if !dest.exists() {
            std::fs::write(&dest, &scaffolded)?;
            report.written += 1;
            continue;
        }
        if !refresh {
            // Plain --with-memories never overwrites an existing file.
            report.unchanged += 1;
            continue;
        }
        let existing = std::fs::read_to_string(&dest)?;
        match memory_refresh_disposition(&existing) {
            MemoryDisposition::UserOwned => report.kept_user += 1,
            MemoryDisposition::Edited => report.kept_edited += 1,
            MemoryDisposition::Pristine => {
                if aida_core::scaffolding::generated_text_matches(&existing, &scaffolded) {
                    report.unchanged += 1;
                } else {
                    std::fs::write(&dest, &scaffolded)?;
                    report.refreshed += 1;
                }
            }
        }
    }

    update_memory_index(mem_dir, &files)?;
    Ok(report)
}

/// Write (or `--refresh`) the starter memory pack into the Claude Code
/// project memory dir for the current working directory. `focus` scopes the
/// pack to a subsystem (STORY-362); `None` writes the full pack.
/// trace:STORY-255 | ai:claude
pub(crate) fn scaffold_memory_pack(refresh: bool, focus: Option<&str>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let home =
        crate::home_dir().context("cannot resolve home directory for the starter memory pack")?;
    let slug = process_probe::encode_cwd_for_projects(&cwd);
    let mem_dir = home
        .join(".claude")
        .join("projects")
        .join(&slug)
        .join("memory");

    let report = scaffold_memory_pack_into(&mem_dir, refresh, focus)?;

    println!();
    if refresh {
        println!("  {}:", "Memory pack refreshed".bold());
        println!(
            "    {} new · {} updated · {} unchanged · {} kept (edited) · {} kept (yours)",
            report.written.to_string().green(),
            report.refreshed.to_string().blue(),
            report.unchanged,
            report.kept_edited.to_string().yellow(),
            report.kept_user,
        );
    } else {
        println!("  {}:", "Starter memory pack".bold());
        println!(
            "    {} written · {} already present",
            report.written.to_string().green(),
            report.unchanged.to_string().yellow(),
        );
    }
    println!("    {}", mem_dir.display().to_string().dimmed());
    // trace:STORY-362 | ai:claude
    if let Some(focus) = focus {
        println!(
            "    {} scoped to subsystem '{}' (universal memories always included)",
            "focus:".dimmed(),
            focus
        );
    }
    Ok(())
}

/// One pack member's relationship to the binary's embedded master.
/// trace:STORY-410 | ai:claude
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum MemoryDriftState {
    /// The master ships this memory; the local dir doesn't have it.
    /// `--refresh` would write it fresh.
    Missing,
    /// Present locally and pristine but byte-different from the current
    /// embedded version — `--refresh` would overlay the newer master.
    Stale,
    /// Present locally, pristine, and byte-identical to the master.
    UpToDate,
    /// Present locally, scaffolded, but the body no longer matches its
    /// recorded checksum — the user edited it. `--refresh` keeps the edit.
    Edited,
    /// Present locally with no `aida-scaffold` marker — the user's own file
    /// shadowing a pack name. `--refresh` never touches it.
    UserOwned,
}

/// One row of `aida memories check` output.
/// trace:STORY-410 | ai:claude
#[derive(Debug)]
pub(crate) struct MemoryDriftRow {
    /// Pack-member filename (e.g. `feedback_advocate_not_be_passive.md`).
    pub(crate) name: String,
    /// `name:` frontmatter label, falling back to the filename.
    pub(crate) label: String,
    /// One-line `description:` from the master template, when present.
    pub(crate) description: String,
    pub(crate) state: MemoryDriftState,
}

/// The full drift report: every embedded pack member classified against the
/// local memory dir. Pure data — printing lives in `print_memory_drift`.
/// trace:STORY-410 | ai:claude
#[derive(Debug, Default)]
pub(crate) struct MemoryDriftReport {
    pub(crate) rows: Vec<MemoryDriftRow>,
}

impl MemoryDriftReport {
    pub(crate) fn count(&self, state: MemoryDriftState) -> usize {
        self.rows.iter().filter(|r| r.state == state).count()
    }
    pub(crate) fn missing(&self) -> usize {
        self.count(MemoryDriftState::Missing)
    }
    pub(crate) fn stale(&self) -> usize {
        self.count(MemoryDriftState::Stale)
    }
    pub(crate) fn up_to_date(&self) -> usize {
        self.count(MemoryDriftState::UpToDate)
    }
    pub(crate) fn edited(&self) -> usize {
        self.count(MemoryDriftState::Edited)
    }
    pub(crate) fn user_owned(&self) -> usize {
        self.count(MemoryDriftState::UserOwned)
    }
    /// How many master-pack members `--refresh` would land (write or overlay).
    /// This is the number that matters for "how behind am I?".
    pub(crate) fn behind(&self) -> usize {
        self.missing() + self.stale()
    }
}

/// Compute the embedded `name:` label + `description:` for a master template.
pub(crate) fn master_memory_meta(template: &str) -> (String, String) {
    let template = normalize_line_endings(template);
    let (fm, _) = split_md_frontmatter(&template).unwrap_or(("", ""));
    let label = frontmatter_field(fm, "name").unwrap_or("").to_string();
    let desc = frontmatter_field(fm, "description")
        .unwrap_or("")
        .trim_matches('"')
        .to_string();
    (label, desc)
}

/// Classify every embedded scaffolding-pack memory against an on-disk memory
/// dir. The pure core of `aida memories check` — takes the dir directly so it
/// is testable without touching the real `$HOME`. Mirrors the marker-driven
/// selection in `scaffold_memory_pack_into` so the two surfaces never diverge.
/// trace:STORY-410 | ai:claude
pub(crate) fn compute_memory_drift_into(mem_dir: &std::path::Path) -> Result<MemoryDriftReport> {
    use aida_core::templates::EMBEDDED_TEMPLATES;

    let mut files: Vec<(&str, &str)> = EMBEDDED_TEMPLATES
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("memories/")
                .filter(|n| *n != "MEMORY.md")
                .map(|n| (n, *v))
        })
        .collect();
    if files.is_empty() {
        anyhow::bail!("no starter memories are embedded in this build");
    }
    files.sort_by(|a, b| a.0.cmp(b.0));

    let mut report = MemoryDriftReport::default();
    for (name, template) in &files {
        let scaffolded = build_scaffolded_memory(template)
            .with_context(|| format!("malformed starter memory template: {name}"))?;
        let (fm_label, description) = master_memory_meta(template);
        let label = if fm_label.is_empty() {
            name.to_string()
        } else {
            fm_label
        };

        let dest = mem_dir.join(name);
        let state = if !dest.exists() {
            MemoryDriftState::Missing
        } else {
            let existing = std::fs::read_to_string(&dest)?;
            match memory_refresh_disposition(&existing) {
                MemoryDisposition::UserOwned => MemoryDriftState::UserOwned,
                MemoryDisposition::Edited => MemoryDriftState::Edited,
                MemoryDisposition::Pristine => {
                    if aida_core::scaffolding::generated_text_matches(&existing, &scaffolded) {
                        MemoryDriftState::UpToDate
                    } else {
                        MemoryDriftState::Stale
                    }
                }
            }
        };

        report.rows.push(MemoryDriftRow {
            name: name.to_string(),
            label,
            description,
            state,
        });
    }
    Ok(report)
}

/// Resolve the Claude Code project memory dir for the current working
/// directory (same slug rule as `scaffold_memory_pack`).
pub(crate) fn project_memory_dir() -> Result<std::path::PathBuf> {
    let cwd = std::env::current_dir()?;
    let home =
        crate::home_dir().context("cannot resolve home directory for the starter memory pack")?;
    let slug = process_probe::encode_cwd_for_projects(&cwd);
    Ok(home
        .join(".claude")
        .join("projects")
        .join(&slug)
        .join("memory"))
}

/// STORY-410 Phase 2: the `aida status` one-liner. Silent unless a local
/// memory pack exists AND is behind the binary's master — so a project that
/// never opted into `--with-memories` is never nagged, and a current pack
/// stays quiet (the line appearing is itself the signal). Best-effort: any
/// error (no $HOME, no embedded pack) degrades to silence rather than
/// breaking the status surface. trace:STORY-410 | ai:claude
pub(crate) fn print_status_memory_drift_section() {
    let Ok(mem_dir) = project_memory_dir() else {
        return;
    };
    // A project that never scaffolded the pack has no memory dir — don't nag.
    if !mem_dir.exists() {
        return;
    }
    let Ok(report) = compute_memory_drift_into(&mem_dir) else {
        return;
    };
    let behind = report.behind();
    if behind == 0 {
        return;
    }
    // Only surface when the pack is actually adopted (some scaffolded files
    // present). An all-missing dir means the user keeps their own memories
    // there but never took the pack — leave them alone.
    if report.up_to_date() + report.stale() + report.edited() == 0 {
        return;
    }
    println!(
        "  {} {} memor{} behind master — run {} for details",
        "Memory pack:".bold().yellow(),
        behind,
        if behind == 1 { "y" } else { "ies" },
        "aida memories check".cyan()
    );
    println!();
}
