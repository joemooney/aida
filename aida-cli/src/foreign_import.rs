//! TASK-724: read-only dry-run PREVIEW of importing neighbor-tool spec
//! artifacts (Spec Kit / OpenSpec / Kiro) into the AIDA graph.
//!
//! This is the SAFE slice of TASK-0416. It parses a directory of foreign
//! markdown artifacts and produces a preview of the records + parent/child/task
//! edges an eventual real importer WOULD create — and writes NOTHING. The
//! actual write-import, stable-ID assignment, and dedup-on-conflict are
//! deferred to the parent (TASK-0416).
//!
//! Design: this module is the PURE core. It takes a filesystem path, reads the
//! foreign artifacts, and returns an `ImportPreview` value. It never touches the
//! AIDA store, never constructs a `Storage`, and never writes back to the
//! source artifacts. `main.rs::handle_import_command` calls `build_preview` and
//! `render_preview`; keeping the graph-shaping logic pure makes it
//! unit-testable with fixtures and guarantees zero graph mutation by
//! construction.
//! trace:TASK-724 | ai:claude

use anyhow::{Context, Result};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Which neighbor-tool layout a directory is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForeignFormat {
    SpecKit,
    OpenSpec,
    Kiro,
}

impl ForeignFormat {
    pub(crate) fn label(self) -> &'static str {
        match self {
            ForeignFormat::SpecKit => "spec-kit",
            ForeignFormat::OpenSpec => "openspec",
            ForeignFormat::Kiro => "kiro",
        }
    }

    /// Parse a user-supplied `--format` value. `auto` returns `None` (meaning
    /// auto-detect). trace:TASK-724
    pub(crate) fn parse_flag(value: &str) -> Result<Option<ForeignFormat>> {
        match value.to_lowercase().as_str() {
            "auto" => Ok(None),
            "spec-kit" | "speckit" => Ok(Some(ForeignFormat::SpecKit)),
            "openspec" => Ok(Some(ForeignFormat::OpenSpec)),
            "kiro" => Ok(Some(ForeignFormat::Kiro)),
            other => {
                anyhow::bail!(
                    "Unknown --format: {other}. Supported: auto, spec-kit, openspec, kiro"
                )
            }
        }
    }
}

/// A single record the importer WOULD create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreviewRecord {
    /// Stable-within-preview local key used to express edges (e.g. "F1",
    /// "F1.T1"). NOT a SPEC-ID — stable-ID assignment is deferred to the parent.
    pub(crate) key: String,
    /// Inferred AIDA requirement type (lowercase, matching the CLI taxonomy).
    pub(crate) inferred_type: String,
    /// Title extracted from the artifact.
    pub(crate) title: String,
    /// Source artifact path (relative to the import root) this record came from.
    pub(crate) source_path: String,
}

/// A parent -> child edge the importer WOULD create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreviewEdge {
    pub(crate) parent_key: String,
    pub(crate) child_key: String,
    /// Edge kind: "parent-child" (feature -> story) or "task" (story -> task).
    pub(crate) kind: String,
}

/// The full read-only preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportPreview {
    pub(crate) format: ForeignFormat,
    pub(crate) records: Vec<PreviewRecord>,
    pub(crate) edges: Vec<PreviewEdge>,
}

/// Auto-detect the format from a directory's layout. trace:TASK-724
fn detect_format(root: &Path) -> Option<ForeignFormat> {
    // OpenSpec: an `openspec/changes/<name>/` tree, or a bare `changes/` dir
    // whose parent dir is named `openspec`.
    if root.join("openspec").join("changes").is_dir()
        || (root.join("changes").is_dir()
            && root.file_name().map(|n| n == "openspec").unwrap_or(false))
    {
        return Some(ForeignFormat::OpenSpec);
    }
    // Spec Kit: a `specs/<feature>/spec.md` tree.
    if root.join("specs").is_dir() {
        return Some(ForeignFormat::SpecKit);
    }
    // Kiro: requirements.md / design.md / tasks.md at the directory root.
    if root.join("requirements.md").is_file()
        || root.join("design.md").is_file()
        || (root.join("tasks.md").is_file() && !root.join("spec.md").is_file())
    {
        return Some(ForeignFormat::Kiro);
    }
    // Spec Kit feature dir passed directly (spec.md present).
    if root.join("spec.md").is_file() {
        return Some(ForeignFormat::SpecKit);
    }
    None
}

/// Build the read-only preview for `root`, optionally with a forced format.
/// Errors cleanly on a missing path or an undetectable layout. trace:TASK-724
pub(crate) fn build_preview(root: &Path, forced: Option<ForeignFormat>) -> Result<ImportPreview> {
    if !root.exists() {
        anyhow::bail!("Import path does not exist: {}", root.display());
    }
    if !root.is_dir() {
        anyhow::bail!(
            "Import path must be a directory of foreign-tool artifacts: {}",
            root.display()
        );
    }

    let format = match forced {
        Some(f) => f,
        None => detect_format(root).with_context(|| {
            format!(
                "Could not auto-detect a Spec Kit / OpenSpec / Kiro layout under {}. \
                 Pass --format to override.",
                root.display()
            )
        })?,
    };

    let preview = match format {
        ForeignFormat::SpecKit => parse_spec_kit(root)?,
        ForeignFormat::OpenSpec => parse_openspec(root)?,
        ForeignFormat::Kiro => parse_kiro(root)?,
    };

    if preview.records.is_empty() {
        anyhow::bail!(
            "No importable artifacts found under {} for format '{}'",
            root.display(),
            format.label()
        );
    }

    Ok(preview)
}

/// First markdown H1/H2 heading in `content`, falling back to a trimmed
/// non-empty first line. trace:TASK-724
fn first_heading(content: &str) -> Option<String> {
    for line in content.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("# ") {
            return Some(rest.trim().to_string());
        }
        if let Some(rest) = t.strip_prefix("## ") {
            return Some(rest.trim().to_string());
        }
    }
    content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.trim_start_matches('#').trim().to_string())
        .filter(|l| !l.is_empty())
}

/// Extract task/checklist bullets from a `tasks.md`-style file. Recognizes
/// markdown task list items (`- [ ]`, `* [x]`) and plain bullets, plus numbered
/// list items. Returns the bullet text (checkbox stripped). trace:TASK-724
fn extract_task_bullets(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        let body = if let Some(rest) = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| t.strip_prefix("+ "))
        {
            rest
        } else if let Some((_, rest)) = t
            .split_once(". ")
            .filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        {
            rest
        } else {
            continue;
        };
        // Strip an optional task-list checkbox.
        let body = body
            .strip_prefix("[ ] ")
            .or_else(|| body.strip_prefix("[x] "))
            .or_else(|| body.strip_prefix("[X] "))
            .unwrap_or(body)
            .trim();
        if !body.is_empty() {
            out.push(body.to_string());
        }
    }
    out
}

/// Relative display path of `p` against `root` (best-effort; falls back to the
/// full path). trace:TASK-724
fn rel(root: &Path, p: &Path) -> String {
    let s = p
        .strip_prefix(root)
        .unwrap_or(p)
        .display()
        .to_string()
        .replace('\\', "/");
    if s.is_empty() {
        ".".to_string()
    } else {
        s
    }
}

/// Spec Kit: `specs/<feature>/spec.md` + optional `plan.md` / `tasks.md`. Each
/// feature dir becomes a parent `feature` record; its `tasks.md` bullets become
/// child `task` records. trace:TASK-724
fn parse_spec_kit(root: &Path) -> Result<ImportPreview> {
    let mut records = Vec::new();
    let mut edges = Vec::new();

    // A single feature dir may be passed directly.
    let feature_dirs: Vec<PathBuf> = if root.join("spec.md").is_file() {
        vec![root.to_path_buf()]
    } else {
        let specs = root.join("specs");
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&specs)
            .with_context(|| format!("reading {}", specs.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        dirs
    };

    let mut feature_idx = 0;
    for fdir in feature_dirs {
        let spec_md = fdir.join("spec.md");
        let title = if spec_md.is_file() {
            std::fs::read_to_string(&spec_md)
                .ok()
                .and_then(|c| first_heading(&c))
        } else {
            None
        }
        .unwrap_or_else(|| {
            fdir.file_name()
                .map(|n| n.to_string_lossy().replace(['-', '_'], " "))
                .unwrap_or_else(|| "feature".to_string())
        });

        feature_idx += 1;
        let fkey = format!("F{feature_idx}");
        let source = if spec_md.is_file() {
            rel(root, &spec_md)
        } else {
            rel(root, &fdir)
        };
        records.push(PreviewRecord {
            key: fkey.clone(),
            inferred_type: "feature".to_string(),
            title,
            source_path: source,
        });

        let tasks_md = fdir.join("tasks.md");
        if tasks_md.is_file() {
            let content = std::fs::read_to_string(&tasks_md).unwrap_or_default();
            for (i, bullet) in extract_task_bullets(&content).into_iter().enumerate() {
                let tkey = format!("{fkey}.T{}", i + 1);
                records.push(PreviewRecord {
                    key: tkey.clone(),
                    inferred_type: "task".to_string(),
                    title: bullet,
                    source_path: rel(root, &tasks_md),
                });
                edges.push(PreviewEdge {
                    parent_key: fkey.clone(),
                    child_key: tkey,
                    kind: "task".to_string(),
                });
            }
        }
    }

    Ok(ImportPreview {
        format: ForeignFormat::SpecKit,
        records,
        edges,
    })
}

/// OpenSpec: `openspec/changes/<name>/` (proposal.md / tasks.md / specs/...).
/// Each change dir becomes a parent `change-request` record; its `tasks.md`
/// bullets become child `task` records. trace:TASK-724
fn parse_openspec(root: &Path) -> Result<ImportPreview> {
    let changes_dir = if root.join("openspec").join("changes").is_dir() {
        root.join("openspec").join("changes")
    } else {
        root.join("changes")
    };

    let mut records = Vec::new();
    let mut edges = Vec::new();

    let mut change_dirs: Vec<PathBuf> = std::fs::read_dir(&changes_dir)
        .with_context(|| format!("reading {}", changes_dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    change_dirs.sort();

    let mut idx = 0;
    for cdir in change_dirs {
        // Title: proposal.md heading, else the dir name.
        let proposal = ["proposal.md", "README.md", "change.md"]
            .iter()
            .map(|f| cdir.join(f))
            .find(|p| p.is_file());
        let title = proposal
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|c| first_heading(&c))
            .unwrap_or_else(|| {
                cdir.file_name()
                    .map(|n| n.to_string_lossy().replace(['-', '_'], " "))
                    .unwrap_or_else(|| "change".to_string())
            });

        idx += 1;
        let ckey = format!("C{idx}");
        let source = proposal
            .as_ref()
            .map(|p| rel(root, p))
            .unwrap_or_else(|| rel(root, &cdir));
        records.push(PreviewRecord {
            key: ckey.clone(),
            inferred_type: "change-request".to_string(),
            title,
            source_path: source,
        });

        let tasks_md = cdir.join("tasks.md");
        if tasks_md.is_file() {
            let content = std::fs::read_to_string(&tasks_md).unwrap_or_default();
            for (i, bullet) in extract_task_bullets(&content).into_iter().enumerate() {
                let tkey = format!("{ckey}.T{}", i + 1);
                records.push(PreviewRecord {
                    key: tkey.clone(),
                    inferred_type: "task".to_string(),
                    title: bullet,
                    source_path: rel(root, &tasks_md),
                });
                edges.push(PreviewEdge {
                    parent_key: ckey.clone(),
                    child_key: tkey,
                    kind: "task".to_string(),
                });
            }
        }
    }

    Ok(ImportPreview {
        format: ForeignFormat::OpenSpec,
        records,
        edges,
    })
}

/// Kiro: `requirements.md` + `design.md` + `tasks.md` at the directory root.
/// `requirements.md` becomes a parent `story` record; `tasks.md` bullets become
/// child `task` records. `design.md` is recorded as a `doc`. trace:TASK-724
fn parse_kiro(root: &Path) -> Result<ImportPreview> {
    let mut records = Vec::new();
    let mut edges = Vec::new();

    let req_md = root.join("requirements.md");
    let parent_key = "S1".to_string();
    let parent_title = if req_md.is_file() {
        std::fs::read_to_string(&req_md)
            .ok()
            .and_then(|c| first_heading(&c))
    } else {
        None
    }
    .unwrap_or_else(|| {
        root.file_name()
            .map(|n| n.to_string_lossy().replace(['-', '_'], " "))
            .unwrap_or_else(|| "feature".to_string())
    });

    records.push(PreviewRecord {
        key: parent_key.clone(),
        inferred_type: "story".to_string(),
        title: parent_title,
        source_path: if req_md.is_file() {
            rel(root, &req_md)
        } else {
            rel(root, root)
        },
    });

    let design_md = root.join("design.md");
    if design_md.is_file() {
        let title = std::fs::read_to_string(&design_md)
            .ok()
            .and_then(|c| first_heading(&c))
            .unwrap_or_else(|| "Design".to_string());
        let dkey = "S1.D1".to_string();
        records.push(PreviewRecord {
            key: dkey.clone(),
            inferred_type: "doc".to_string(),
            title,
            source_path: rel(root, &design_md),
        });
        edges.push(PreviewEdge {
            parent_key: parent_key.clone(),
            child_key: dkey,
            kind: "parent-child".to_string(),
        });
    }

    let tasks_md = root.join("tasks.md");
    if tasks_md.is_file() {
        let content = std::fs::read_to_string(&tasks_md).unwrap_or_default();
        for (i, bullet) in extract_task_bullets(&content).into_iter().enumerate() {
            let tkey = format!("{parent_key}.T{}", i + 1);
            records.push(PreviewRecord {
                key: tkey.clone(),
                inferred_type: "task".to_string(),
                title: bullet,
                source_path: rel(root, &tasks_md),
            });
            edges.push(PreviewEdge {
                parent_key: parent_key.clone(),
                child_key: tkey,
                kind: "task".to_string(),
            });
        }
    }

    Ok(ImportPreview {
        format: ForeignFormat::Kiro,
        records,
        edges,
    })
}

/// Render the preview as human-readable text. The caller prints this; this
/// function builds NO connection to the store. trace:TASK-724
pub(crate) fn render_preview(root: &Path, preview: &ImportPreview) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "DRY RUN — preview only, NOTHING written to the graph or to source artifacts."
    );
    let _ = writeln!(
        s,
        "Source: {}  (detected format: {})",
        root.display(),
        preview.format.label()
    );
    let _ = writeln!(s);
    let _ = writeln!(s, "Would create {} record(s):", preview.records.len());
    for r in &preview.records {
        let _ = writeln!(s, "  [{}] {:<14} {}", r.key, r.inferred_type, r.title);
        let _ = writeln!(
            s,
            "       source: {}  (provenance: {})",
            r.source_path,
            preview.format.label()
        );
    }

    let _ = writeln!(s);
    if preview.edges.is_empty() {
        let _ = writeln!(s, "Would create 0 edge(s).");
    } else {
        let _ = writeln!(s, "Would create {} edge(s):", preview.edges.len());
        for e in &preview.edges {
            let _ = writeln!(s, "  {} --{}--> {}", e.parent_key, e.kind, e.child_key);
        }
    }

    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "No graph mutation performed. Run without --dry-run once the real importer ships."
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(p: &Path, content: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, content).unwrap();
    }

    #[test]
    fn parse_flag_maps_names_and_auto() {
        assert_eq!(ForeignFormat::parse_flag("auto").unwrap(), None);
        assert_eq!(
            ForeignFormat::parse_flag("spec-kit").unwrap(),
            Some(ForeignFormat::SpecKit)
        );
        assert_eq!(
            ForeignFormat::parse_flag("OpenSpec").unwrap(),
            Some(ForeignFormat::OpenSpec)
        );
        assert_eq!(
            ForeignFormat::parse_flag("kiro").unwrap(),
            Some(ForeignFormat::Kiro)
        );
        assert!(ForeignFormat::parse_flag("garbage").is_err());
    }

    #[test]
    fn first_heading_prefers_h1() {
        assert_eq!(
            first_heading("intro\n# Real Title\n## sub"),
            Some("Real Title".to_string())
        );
        assert_eq!(
            first_heading("## Only sub\nbody"),
            Some("Only sub".to_string())
        );
        assert_eq!(
            first_heading("plain first line\nmore"),
            Some("plain first line".to_string())
        );
    }

    #[test]
    fn extract_task_bullets_handles_checkboxes_and_numbers() {
        let content = "\
# Tasks
- [ ] Build the parser
* [x] Wire the CLI flag
1. Add a test
- plain bullet
not a bullet
";
        assert_eq!(
            extract_task_bullets(content),
            vec![
                "Build the parser".to_string(),
                "Wire the CLI flag".to_string(),
                "Add a test".to_string(),
                "plain bullet".to_string(),
            ]
        );
    }

    #[test]
    fn missing_path_errors_cleanly() {
        let err = build_preview(Path::new("/no/such/dir/xyz"), None).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn garbage_dir_errors_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("README.txt"), "nothing useful here");
        let err = build_preview(tmp.path(), None).unwrap_err();
        assert!(err.to_string().contains("auto-detect"));
    }

    #[test]
    fn spec_kit_detects_and_previews_records_and_edges() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("specs/user-auth/spec.md"),
            "# User Authentication\nUsers can log in.",
        );
        write(
            &root.join("specs/user-auth/tasks.md"),
            "# Tasks\n- [ ] Add login form\n- [ ] Validate credentials\n",
        );

        // Snapshot the dir tree to prove nothing is written/overwritten.
        let before = dir_snapshot(root);

        let preview = build_preview(root, None).unwrap();
        assert_eq!(preview.format, ForeignFormat::SpecKit);
        // 1 feature + 2 tasks.
        assert_eq!(preview.records.len(), 3);
        assert_eq!(preview.records[0].inferred_type, "feature");
        assert_eq!(preview.records[0].title, "User Authentication");
        assert_eq!(preview.records[1].inferred_type, "task");
        assert_eq!(preview.records[1].title, "Add login form");
        // 2 task edges.
        assert_eq!(preview.edges.len(), 2);
        assert_eq!(preview.edges[0].kind, "task");
        assert_eq!(preview.edges[0].parent_key, "F1");

        // Render does not panic and mentions dry-run.
        let out = render_preview(root, &preview);
        assert!(out.contains("DRY RUN"));
        assert!(out.contains("User Authentication"));

        // ZERO mutation: the source tree is byte-for-byte identical.
        let after = dir_snapshot(root);
        assert_eq!(
            before, after,
            "dry-run must not write or overwrite anything"
        );
    }

    #[test]
    fn openspec_detects_change_and_tasks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("openspec/changes/add-search/proposal.md"),
            "# Add full-text search\nWhy we need it.",
        );
        write(
            &root.join("openspec/changes/add-search/tasks.md"),
            "- [ ] Index documents\n- [ ] Add query endpoint\n",
        );

        let preview = build_preview(root, None).unwrap();
        assert_eq!(preview.format, ForeignFormat::OpenSpec);
        assert_eq!(preview.records[0].inferred_type, "change-request");
        assert_eq!(preview.records[0].title, "Add full-text search");
        assert_eq!(preview.edges.len(), 2);
    }

    #[test]
    fn kiro_detects_requirements_design_tasks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("requirements.md"),
            "# Checkout flow\nAs a user...",
        );
        write(&root.join("design.md"), "# Checkout design\n...");
        write(&root.join("tasks.md"), "1. Build cart\n2. Build payment\n");

        let preview = build_preview(root, None).unwrap();
        assert_eq!(preview.format, ForeignFormat::Kiro);
        assert_eq!(preview.records[0].inferred_type, "story");
        assert_eq!(preview.records[0].title, "Checkout flow");
        // story + design doc + 2 tasks.
        assert_eq!(preview.records.len(), 4);
        assert!(preview
            .records
            .iter()
            .any(|r| r.inferred_type == "doc" && r.title == "Checkout design"));
        // 1 design parent-child + 2 task edges.
        assert_eq!(preview.edges.len(), 3);
    }

    #[test]
    fn format_override_forces_kiro_even_with_specs_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("requirements.md"), "# Forced\nbody");
        // Also a specs/ dir that would auto-detect as spec-kit.
        write(&root.join("specs/x/spec.md"), "# X");

        let auto = build_preview(root, None).unwrap();
        assert_eq!(auto.format, ForeignFormat::SpecKit);

        let forced = build_preview(root, Some(ForeignFormat::Kiro)).unwrap();
        assert_eq!(forced.format, ForeignFormat::Kiro);
        assert_eq!(forced.records[0].title, "Forced");
    }

    /// A sorted list of (relative-path, content) for every file under `root`.
    fn dir_snapshot(root: &Path) -> Vec<(String, String)> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
            let mut entries: Vec<_> = fs::read_dir(dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .collect();
            entries.sort();
            for p in entries {
                if p.is_dir() {
                    walk(&p, root, out);
                } else {
                    out.push((rel(root, &p), fs::read_to_string(&p).unwrap_or_default()));
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out.sort();
        out
    }
}
