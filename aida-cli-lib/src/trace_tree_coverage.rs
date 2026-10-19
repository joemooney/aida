//! `aida trace coverage --all | --path <PATH>` — whole-tree function trace
//! coverage (TASK-1783, slice 1 of STORY-1672).
//!
//! The diff-scoped `aida trace coverage` asks "is the code this branch
//! changed traced?". This mode asks the gap-detector question instead: which
//! Rust functions does no spec claim? A function is traced when a live
//! `trace:<SPEC-ID>` marker (the diff mode's anchor grammar and resolution)
//! claims it under the shared `trace_regions` rule: a file `//!` anchor, a
//! marker on its enclosing `impl`, or one inside it or in its leading
//! prefix. `aida lint constants` uses the same rule. Files are filtered by
//! the same SPIKE-47 file exemptions. Report-only.
// trace:TASK-1783 | ai:claude

use anyhow::Result;
use serde::Serialize;

use crate::cli::TraceCoverageFailOn;
use crate::trace::{resolve_spec_in_store, SpecResolution};
use crate::trace_regions::{traced_file, ItemKind};
use crate::{find_project_root, load_store_for_lookup};

/// One untraced function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FnCoverage {
    /// `name`, or `Type::name` for methods and trait default methods.
    pub(crate) name: String,
    /// 1-based line of the `fn` item (its first attribute, if any).
    pub(crate) line: usize,
}

/// Per-file result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FileCoverage {
    pub(crate) file: String,
    pub(crate) functions: usize,
    pub(crate) traced: usize,
    pub(crate) untraced: Vec<FnCoverage>,
    /// The file carries a live module-level `//!` anchor, which traces every
    /// function in it.
    pub(crate) file_anchor: bool,
}

/// Whole-run result.
#[derive(Debug, Default, Serialize)]
pub(crate) struct TreeCoverage {
    pub(crate) files: Vec<FileCoverage>,
    /// Rust files that did not parse and so were not counted.
    pub(crate) unparsed: Vec<String>,
}

impl TreeCoverage {
    pub(crate) fn functions(&self) -> usize {
        self.files.iter().map(|f| f.functions).sum()
    }
    pub(crate) fn traced(&self) -> usize {
        self.files.iter().map(|f| f.traced).sum()
    }
    pub(crate) fn untraced(&self) -> usize {
        self.functions() - self.traced()
    }
}

/// The PURE core: classify every function in one Rust source text. `resolve`
/// decides whether a marker's id is live (the diff mode's resolver).
/// `None` when the text does not parse as a Rust file.
pub(crate) fn file_coverage<F>(file: &str, text: &str, resolve: &mut F) -> Option<FileCoverage>
where
    F: FnMut(&str) -> SpecResolution,
{
    let traced_regions = traced_file(text, |id| matches!(resolve(id), SpecResolution::Live))?;
    let mut traced = 0;
    let mut untraced = Vec::new();
    for item in traced_regions
        .items
        .iter()
        .filter(|i| i.kind == ItemKind::Fn)
    {
        if traced_regions.specs_at(item.range.start).is_empty() {
            untraced.push(FnCoverage {
                name: item.name.clone(),
                line: item.range.start + 1,
            });
        } else {
            traced += 1;
        }
    }
    Some(FileCoverage {
        file: file.to_string(),
        functions: traced + untraced.len(),
        traced,
        untraced,
        file_anchor: !traced_regions.file_specs.is_empty(),
    })
}

/// Route `aida trace coverage`: whole-tree mode with `--all` / `--path`,
/// otherwise the unchanged diff-scoped mode.
// trace:TASK-1783 | ai:claude
pub(crate) fn dispatch_coverage(
    range: Option<&str>,
    json: bool,
    block: bool,
    all: bool,
    path: Option<&std::path::Path>,
    fail_on: Option<TraceCoverageFailOn>,
) -> Result<()> {
    if all || path.is_some() {
        return handle_tree_coverage(path, json, fail_on);
    }
    anyhow::ensure!(
        fail_on.is_none(),
        "--fail-on applies to whole-tree coverage; pass --all or --path (the diff mode uses --block)"
    );
    crate::handle_trace_coverage(range, json, block)
}

/// `aida trace coverage --all | --path <PATH> [--json] [--fail-on uncovered]`.
// trace:TASK-1783 | ai:claude
pub(crate) fn handle_tree_coverage(
    path: Option<&std::path::Path>,
    json: bool,
    fail_on: Option<TraceCoverageFailOn>,
) -> Result<()> {
    use colored::Colorize;

    let project_root = find_project_root()?;
    let target = match path {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        Some(p) => std::env::current_dir()?.join(p),
        None => project_root.clone(),
    };
    anyhow::ensure!(target.exists(), "{} does not exist", target.display());

    let store = load_store_for_lookup(&project_root);
    if store.is_none() {
        eprintln!(
            "{} trace coverage: no requirement store reachable from {} — every trace id is \
             treated as live, so coverage may be over-counted.",
            crate::glyph(crate::glyphs::Glyph::Warning),
            project_root.display()
        );
    }
    let mut resolve = |id: &str| resolve_spec_in_store(store.as_ref(), id);

    let mut files = Vec::new();
    if target.is_file() {
        files.push(target.clone());
    } else {
        crate::trace::collect_source_files(&target, &["rs"], &mut files);
    }
    files.sort();

    let mut report = TreeCoverage::default();
    for file in files {
        let rel = file
            .strip_prefix(&project_root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        // SPIKE-47 file exemptions, probing the same 5-line generated header
        // as the diff mode.
        let header: String = text.lines().take(5).collect::<Vec<_>>().join("\n");
        if crate::trace::classify_coverage_file(&rel, Some(&header)).is_some() {
            continue;
        }
        match file_coverage(&rel, &text, &mut resolve) {
            Some(fc) if fc.functions > 0 => report.files.push(fc),
            Some(_) => {}
            None => report.unparsed.push(rel),
        }
    }

    let functions = report.functions();
    let traced = report.traced();
    let ratio = if functions == 0 {
        1.0
    } else {
        traced as f64 / functions as f64
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "mode": "tree",
                "files": report.files.len(),
                "functions": functions,
                "traced": traced,
                "untraced": report.untraced(),
                "ratio": ratio,
                "unparsed": report.unparsed,
                "per_file": report.files,
            }))?
        );
    } else {
        for f in report.files.iter().filter(|f| !f.untraced.is_empty()) {
            println!("{}  {}/{}", f.file.bold(), f.traced, f.functions);
            for u in &f.untraced {
                println!("    {}:{}  {}", f.file, u.line, u.name.yellow());
            }
        }
        for u in &report.unparsed {
            println!("{} {u}: did not parse, not counted", "skipped".dimmed());
        }
        let summary = format!(
            "{traced}/{functions} function(s) traced ({:.1}%) across {} file(s); {} untraced.",
            ratio * 100.0,
            report.files.len(),
            report.untraced()
        );
        if report.untraced() == 0 {
            println!("{}", summary.green());
        } else {
            println!("{summary}");
        }
    }

    if fail_on == Some(TraceCoverageFailOn::Uncovered) && report.untraced() > 0 {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_live(_: &str) -> SpecResolution {
        SpecResolution::Live
    }

    // The signed acceptance fixture: one traced and one untraced fn -> 1/2,
    // naming the untraced one.
    // trace:TASK-1783.accb5051 | ai:claude
    #[test]
    fn one_traced_one_untraced_reports_half() {
        let src = "\
// trace:STORY-1 | ai:claude
pub fn traced() {}

pub fn untraced() {}
";
        let fc = file_coverage("src/lib.rs", src, &mut all_live).unwrap();
        assert_eq!(fc.functions, 2);
        assert_eq!(fc.traced, 1);
        assert_eq!(fc.untraced.len(), 1);
        assert_eq!(fc.untraced[0].name, "untraced");
        assert_eq!(fc.untraced[0].line, 4);
    }

    // trace:TASK-1783.accb5051 | ai:claude
    #[test]
    fn marker_inside_or_in_the_leading_prefix_counts() {
        let src = "\
/// Docs.
// trace:STORY-1 | ai:claude
#[inline]
fn above() {}

fn inside() {
    // trace:STORY-2 | ai:claude
}

// trace:STORY-3 | ai:claude

fn across_blank() {}
fn after_code() {}

fn string_is_not_a_marker() { let _ = \"trace:STORY-4\"; }
";
        let fc = file_coverage("f.rs", src, &mut all_live).unwrap();
        let names: Vec<&str> = fc.untraced.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(names, vec!["after_code", "string_is_not_a_marker"]);
        assert_eq!(fc.traced, 3);
    }

    #[test]
    fn impl_marker_covers_methods_but_not_siblings_of_a_traced_method() {
        let src = "\
// trace:STORY-1 | ai:claude
impl A {
    fn a1(&self) {}
}
impl B {
    // trace:STORY-2 | ai:claude
    fn b1(&self) {}
    fn b2(&self) {}
}
";
        let fc = file_coverage("f.rs", src, &mut all_live).unwrap();
        let names: Vec<&str> = fc.untraced.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(names, vec!["B::b2"]);
    }

    // trace:TASK-1783.ac438744 | ai:claude
    #[test]
    fn live_file_anchor_traces_every_function() {
        let src = "\
//! Module. trace:STORY-1 | ai:claude

fn a() {}
impl S { fn b(&self) {} }
";
        let fc = file_coverage("f.rs", src, &mut all_live).unwrap();
        assert!(fc.file_anchor);
        assert_eq!((fc.traced, fc.functions), (2, 2));
    }

    #[test]
    fn methods_are_named_by_owner_and_bodiless_trait_fns_skipped() {
        let src = "\
impl<T> Wrapper<T> {
    fn method(&self) {}
}
trait Shape {
    fn area(&self) -> f64;
    fn twice(&self) -> f64 { 2.0 * self.area() }
}
";
        let fc = file_coverage("f.rs", src, &mut all_live).unwrap();
        let names: Vec<&str> = fc.untraced.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(names, vec!["Wrapper::method", "Shape::twice"]);
    }

    #[test]
    fn dead_markers_do_not_cover() {
        let src = "\
//! Module. trace:STORY-405 | ai:claude

// trace:STORY-404 | ai:claude
fn dangling() {}
";
        let mut resolve = |_: &str| SpecResolution::Missing;
        let fc = file_coverage("f.rs", src, &mut resolve).unwrap();
        assert!(!fc.file_anchor);
        assert_eq!(fc.traced, 0);
        assert_eq!(fc.untraced[0].name, "dangling");
    }

    #[test]
    fn unparsable_source_is_none() {
        assert!(file_coverage("f.rs", "fn broken( {", &mut all_live).is_none());
    }
}
