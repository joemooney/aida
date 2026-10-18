//! `aida lint constants` — numeric literals in traced Rust code that the
//! owning spec never explains (TASK-1785, slice 2 of STORY-1672).
//!
//! A traced region is a Rust function or `const`/`static` item with a
//! `trace:<SPEC-ID>` marker in its comments or in the comment block directly
//! above it. Every integer/float literal inside such a region is checked
//! against the owning specs: it is explained when its normalized value
//! appears in a spec's description or in a decision linked to that spec.
//! Only 0, 1 and literals on a line carrying `// arbitrary` are exempt.
//! Report-only: nothing here writes to the store.
// trace:TASK-1785 | ai:claude

use std::collections::{BTreeSet, HashSet};

use anyhow::Result;
use serde::Serialize;
use syn::{spanned::Spanned, visit::Visit};

use crate::cli::LintConstantsFailOn;
use crate::{find_project_root, load_store_for_lookup};

/// One numeric literal found inside a traced region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TracedLiteral {
    /// 1-based source line.
    pub(crate) line: usize,
    /// The literal as written (`0x0F`, `4_096u64`).
    pub(crate) literal: String,
    /// The normalized value it is compared by (`15`, `4096`).
    pub(crate) value: String,
    /// Owning spec ids from every enclosing traced region.
    pub(crate) specs: Vec<String>,
    /// `true` for 0/1 and `// arbitrary`-marked lines.
    pub(crate) exempt: bool,
}

/// An unexplained literal, as reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ConstantFinding {
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) literal: String,
    pub(crate) value: String,
    pub(crate) specs: Vec<String>,
}

/// Whole-run totals plus the findings.
#[derive(Debug, Default, Serialize)]
pub(crate) struct ConstantsReport {
    pub(crate) files_scanned: usize,
    /// Rust files that did not parse and so were not checked.
    pub(crate) unparsed: Vec<String>,
    pub(crate) literals_checked: usize,
    pub(crate) explained: usize,
    pub(crate) exempt: usize,
    pub(crate) findings: Vec<ConstantFinding>,
}

/// Normalize a number for comparison: integers by value in decimal (hex,
/// octal and binary included), floats with an integral value as that integer,
/// other floats in Rust's shortest round-trip form. `_` separators and type
/// suffixes are expected to be stripped already. PURE.
fn normalize_number(digits: &str, radix: u32, is_float: bool) -> Option<String> {
    if !is_float {
        return u128::from_str_radix(digits, radix)
            .ok()
            .map(|v| v.to_string());
    }
    let v: f64 = digits.parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    // Below 2^53 an integral f64 is exact, so 15.0 compares equal to 15.
    if v.fract() == 0.0 && v.abs() < 9_007_199_254_740_992.0 {
        return Some(format!("{}", v as i128));
    }
    Some(format!("{v}"))
}

/// Normalized value of a Rust numeric literal token, or `None` for any other
/// literal kind (strings, chars, bytes, bools). PURE.
pub(crate) fn normalize_rust_literal(lit: &syn::Lit) -> Option<String> {
    match lit {
        // syn's base10 digits already drop `_`, the suffix and the radix.
        syn::Lit::Int(i) => normalize_number(i.base10_digits(), 10, false),
        syn::Lit::Float(f) => normalize_number(f.base10_digits(), 10, true),
        _ => None,
    }
}

/// Every number mentioned in free text, normalized the same way as literals:
/// decimal (with `_` or `,`-thousands separators), decimals with a fraction,
/// and `0x`/`0o`/`0b` forms. PURE.
pub(crate) fn numbers_in_text(text: &str) -> HashSet<String> {
    let re = regex::Regex::new(
        r"(?i)\b0x[0-9a-f_]+\b|\b0o[0-7_]+\b|\b0b[01_]+\b|\d{1,3}(?:,\d{3})+(?:\.\d+)?|\d[\d_]*(?:\.\d+)?",
    )
    .unwrap();
    let mut out = HashSet::new();
    for m in re.find_iter(text) {
        let raw = m.as_str().replace(['_', ','], "");
        let lower = raw.to_ascii_lowercase();
        let value = if let Some(hex) = lower.strip_prefix("0x") {
            normalize_number(hex, 16, false)
        } else if let Some(oct) = lower.strip_prefix("0o") {
            normalize_number(oct, 8, false)
        } else if let Some(bin) = lower.strip_prefix("0b") {
            normalize_number(bin, 2, false)
        } else if raw.contains('.') {
            normalize_number(&raw, 10, true)
        } else {
            normalize_number(&raw, 10, false)
        };
        if let Some(v) = value {
            out.insert(v);
        }
    }
    out
}

/// True when a line's comment text carries the `// arbitrary` marker
/// (optionally `// arbitrary: <reason>`). PURE.
fn is_arbitrary_marker(comment: &str) -> bool {
    let re = regex::Regex::new(r"//\s*arbitrary(?:\s*:|\s|$)").unwrap();
    re.is_match(comment)
}

/// Line ranges (0-based, end-exclusive) of the items that can be traced
/// regions: functions, methods, trait default methods, consts and statics.
#[derive(Default)]
struct Regions(Vec<std::ops::Range<usize>>);

impl Regions {
    fn push(&mut self, span: proc_macro2::Span) {
        self.0
            .push(span.start().line.saturating_sub(1)..span.end().line);
    }
}

impl<'ast> Visit<'ast> for Regions {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.push(node.span());
        syn::visit::visit_item_fn(self, node);
    }
    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.push(node.span());
        syn::visit::visit_impl_item_fn(self, node);
    }
    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        if node.default.is_some() {
            self.push(node.span());
        }
        syn::visit::visit_trait_item_fn(self, node);
    }
    fn visit_item_const(&mut self, node: &'ast syn::ItemConst) {
        self.push(node.span());
        syn::visit::visit_item_const(self, node);
    }
    fn visit_impl_item_const(&mut self, node: &'ast syn::ImplItemConst) {
        self.push(node.span());
        syn::visit::visit_impl_item_const(self, node);
    }
    fn visit_item_static(&mut self, node: &'ast syn::ItemStatic) {
        self.push(node.span());
        syn::visit::visit_item_static(self, node);
    }
}

/// Collect every literal token, macro bodies included (syn does not parse
/// macro arguments, so walk the raw token stream).
fn literal_tokens(stream: proc_macro2::TokenStream, out: &mut Vec<proc_macro2::Literal>) {
    for tree in stream {
        match tree {
            proc_macro2::TokenTree::Group(g) => literal_tokens(g.stream(), out),
            proc_macro2::TokenTree::Literal(l) => out.push(l),
            _ => {}
        }
    }
}

/// The PURE core: every numeric literal inside a traced region of one Rust
/// source text, with its owning specs and exemption. `None` when the text
/// does not parse as a Rust file.
pub(crate) fn scan_rust_source(text: &str) -> Option<Vec<TracedLiteral>> {
    let file = syn::parse_file(text).ok()?;
    let comments = crate::mandate_traces::rust_comments(text)?;
    let lines: Vec<&str> = text.lines().collect();
    let anchor = crate::trace::coverage_anchor_re();

    let mut regions = Regions::default();
    regions.visit_file(&file);

    // Each region owns the markers in its span plus in the uninterrupted
    // comment block directly above it (plain `//` comments sit outside
    // syn's item span; doc comments and attributes are already inside it).
    let owned: Vec<(std::ops::Range<usize>, BTreeSet<String>)> = regions
        .0
        .into_iter()
        .filter_map(|range| {
            let mut start = range.start;
            while start > 0 {
                let i = start - 1;
                let comment = comments.get(i).map(|(_, c)| c.trim()).unwrap_or("");
                if !comment.is_empty() && lines.get(i).map(|l| l.trim()) == Some(comment) {
                    start -= 1;
                } else {
                    break;
                }
            }
            let specs: BTreeSet<String> = (start..range.end)
                .filter_map(|i| comments.get(i))
                .flat_map(|(_, c)| {
                    anchor
                        .captures_iter(c)
                        .map(|cap| cap[1].to_ascii_uppercase())
                        .collect::<Vec<_>>()
                })
                .collect();
            (!specs.is_empty()).then_some((range, specs))
        })
        .collect();
    if owned.is_empty() {
        return Some(Vec::new());
    }

    let mut tokens = Vec::new();
    literal_tokens(text.parse().ok()?, &mut tokens);
    let mut out = Vec::new();
    for token in tokens {
        let line0 = token.span().start().line.saturating_sub(1);
        let specs: BTreeSet<String> = owned
            .iter()
            .filter(|(r, _)| r.contains(&line0))
            .flat_map(|(_, s)| s.iter().cloned())
            .collect();
        if specs.is_empty() {
            continue;
        }
        let lit = syn::Lit::new(token.clone());
        let Some(value) = normalize_rust_literal(&lit) else {
            continue;
        };
        let marked = comments
            .get(line0)
            .is_some_and(|(_, c)| is_arbitrary_marker(c));
        out.push(TracedLiteral {
            line: line0 + 1,
            literal: token.to_string(),
            exempt: value == "0" || value == "1" || marked,
            value,
            specs: specs.into_iter().collect(),
        });
    }
    out.sort_by_key(|l| l.line);
    Some(out)
}

/// The numbers each owning spec explains: its description plus the title
/// and description of every decision linked to it in either direction.
fn explaining_numbers(store: &aida_core::RequirementsStore, spec_id: &str) -> HashSet<String> {
    let Some(spec) = store.requirements.iter().find(|r| {
        r.spec_id
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case(spec_id))
            || r.agreed_id
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(spec_id))
    }) else {
        return HashSet::new();
    };
    let mut text = spec.description.clone();
    for d in store
        .requirements
        .iter()
        .filter(|r| r.req_type == aida_core::RequirementType::Decision)
    {
        let linked = spec.relationships.iter().any(|rel| rel.target_id == d.id)
            || d.relationships.iter().any(|rel| rel.target_id == spec.id);
        if linked {
            text.push('\n');
            text.push_str(&d.title);
            text.push('\n');
            text.push_str(&d.description);
        }
    }
    numbers_in_text(&text)
}

/// Fold one file's traced literals into the report, resolving explanations
/// through `explains(spec_id)`. PURE (resolver injected).
pub(crate) fn tally_file<F>(
    report: &mut ConstantsReport,
    file: &str,
    literals: Vec<TracedLiteral>,
    explains: &mut F,
) where
    F: FnMut(&str) -> HashSet<String>,
{
    for lit in literals {
        report.literals_checked += 1;
        if lit.exempt {
            report.exempt += 1;
            continue;
        }
        if lit.specs.iter().any(|s| explains(s).contains(&lit.value)) {
            report.explained += 1;
            continue;
        }
        report.findings.push(ConstantFinding {
            file: file.to_string(),
            line: lit.line,
            literal: lit.literal,
            value: lit.value,
            specs: lit.specs,
        });
    }
}

/// `aida lint constants [--path <PATH>] [--json] [--fail-on unexplained]`.
// trace:TASK-1785 | ai:claude
pub(crate) fn handle_lint_constants(
    path: Option<&std::path::Path>,
    json: bool,
    fail_on: Option<LintConstantsFailOn>,
) -> Result<()> {
    use colored::Colorize;

    let project_root = find_project_root()?;
    let store = load_store_for_lookup(&project_root)
        .ok_or_else(|| anyhow::anyhow!("could not load the AIDA requirements store"))?;
    let target = match path {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        Some(p) => std::env::current_dir()?.join(p),
        None => project_root.clone(),
    };
    anyhow::ensure!(target.exists(), "{} does not exist", target.display());

    let mut files = Vec::new();
    if target.is_file() {
        files.push(target.clone());
    } else {
        crate::trace::collect_source_files(&target, &["rs"], &mut files);
    }
    files.sort();

    let mut cache: std::collections::HashMap<String, HashSet<String>> = Default::default();
    let mut explains = |id: &str| {
        cache
            .entry(id.to_string())
            .or_insert_with(|| explaining_numbers(&store, id))
            .clone()
    };
    let mut report = ConstantsReport::default();
    for file in files {
        let rel = file
            .strip_prefix(&project_root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        // SPIKE-47 file exemptions (tests, generated, vendored) as in
        // `aida trace coverage`, probing the same 5-line generated header.
        let header: String = text.lines().take(5).collect::<Vec<_>>().join("\n"); // arbitrary: matches trace coverage
        if crate::trace::classify_coverage_file(&rel, Some(&header)).is_some() {
            continue;
        }
        report.files_scanned += 1;
        match scan_rust_source(&text) {
            Some(literals) => tally_file(&mut report, &rel, literals, &mut explains),
            None => report.unparsed.push(rel),
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "lint": "constants",
                "files_scanned": report.files_scanned,
                "unparsed": report.unparsed,
                "literals_checked": report.literals_checked,
                "explained": report.explained,
                "exempt": report.exempt,
                "unexplained": report.findings.len(),
                "findings": report.findings,
            }))?
        );
    } else {
        for f in &report.findings {
            println!(
                "{}:{}  {}  {}",
                f.file,
                f.line,
                f.literal.yellow(),
                format!("(not in {})", f.specs.join(", ")).dimmed()
            );
        }
        for u in &report.unparsed {
            println!("{} {u}: did not parse, not checked", "skipped".dimmed());
        }
        let summary = format!(
            "{} unexplained of {} literal(s) in traced code across {} file(s) \
             ({} explained, {} exempt).",
            report.findings.len(),
            report.literals_checked,
            report.files_scanned,
            report.explained,
            report.exempt
        );
        if report.findings.is_empty() {
            println!("{}", summary.green());
        } else {
            println!("{summary}");
            println!(
                "{}",
                "Record each value in the spec, or mark the line `// arbitrary: <reason>`."
                    .dimmed()
            );
        }
    }

    if fail_on == Some(LintConstantsFailOn::Unexplained) && !report.findings.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn explains_15(id: &str) -> HashSet<String> {
        if id == "STORY-1" {
            numbers_in_text("Pause when io-full reaches 15.")
        } else {
            HashSet::new()
        }
    }

    // The signed acceptance fixture: spec mentions 15; code has 15, 40 and a
    // marked 7 -> only 40 is flagged.
    #[test]
    fn only_the_unmentioned_literal_is_flagged() {
        let src = "\
// trace:STORY-1 | ai:claude
fn throttle(io: u32, cpu: u32) -> bool {
    let seven = 7; // arbitrary: retry budget
    io > 15 || cpu > 40 || seven == 0
}
";
        let lits = scan_rust_source(src).unwrap();
        let mut report = ConstantsReport::default();
        tally_file(&mut report, "src/lib.rs", lits, &mut explains_15);
        assert_eq!(
            report.findings,
            vec![ConstantFinding {
                file: "src/lib.rs".into(),
                line: 4,
                literal: "40".into(),
                value: "40".into(),
                specs: vec!["STORY-1".into()],
            }]
        );
        assert_eq!(report.literals_checked, 4);
        assert_eq!(report.explained, 1);
        // The marked 7 and the 0.
        assert_eq!(report.exempt, 2);
    }

    #[test]
    fn untraced_code_is_not_scanned() {
        let src = "\
// trace:STORY-1 | ai:claude
fn traced() -> u32 { 40 }

fn untraced() -> u32 { 99 }

// an ordinary comment
const LIMIT: u32 = 98;
";
        let lits = scan_rust_source(src).unwrap();
        assert_eq!(lits.len(), 1);
        assert_eq!(lits[0].value, "40");
    }

    #[test]
    fn marker_must_be_directly_above_or_inside() {
        let src = "\
// trace:STORY-1 | ai:claude

fn separated() -> u32 { 40 }

/// Doc comment.
#[inline]
fn inside() -> u32 {
    // trace:STORY-2 | ai:claude
    41
}
";
        let lits = scan_rust_source(src).unwrap();
        assert_eq!(lits.len(), 1);
        assert_eq!(lits[0].value, "41");
        assert_eq!(lits[0].specs, vec!["STORY-2".to_string()]);
    }

    #[test]
    fn literals_compare_by_normalized_value() {
        let src = "\
// trace:STORY-1 | ai:claude
const SIZES: [u64; 4] = [0x0F, 1_5u64, 15.0 as u64, 4_096];
";
        let lits = scan_rust_source(src).unwrap();
        let values: Vec<&str> = lits.iter().map(|l| l.value.as_str()).collect();
        // `4` is the array length.
        assert_eq!(values, vec!["4", "15", "15", "15", "4096"]);
        let mut report = ConstantsReport::default();
        tally_file(&mut report, "f.rs", lits, &mut |_| {
            numbers_in_text("width 4, rotate at 4,096 or 0xf")
        });
        assert!(report.findings.is_empty(), "{:?}", report.findings);
    }

    #[test]
    fn zero_one_exempt_and_strings_ignored() {
        let src = "\
// trace:STORY-1 | ai:claude
fn f() -> (u8, f64, &'static str) { (1, 0.0, \"40 // arbitrary\") }
";
        let lits = scan_rust_source(src).unwrap();
        assert_eq!(lits.len(), 2);
        assert!(lits.iter().all(|l| l.exempt));
    }

    #[test]
    fn macro_arguments_inside_methods_are_scanned() {
        let src = "\
impl S {
    // trace:STORY-1 | ai:claude
    fn f(&self) { println!(\"{}\", 50); }
}
";
        let lits = scan_rust_source(src).unwrap();
        assert_eq!(lits.len(), 1);
        assert_eq!(lits[0].value, "50");
    }

    #[test]
    fn arbitrary_marker_shapes() {
        assert!(is_arbitrary_marker("   // arbitrary"));
        assert!(is_arbitrary_marker("// arbitrary: picked by eye"));
        assert!(is_arbitrary_marker("//arbitrary"));
        assert!(!is_arbitrary_marker("// arbitrarily chosen"));
        assert!(!is_arbitrary_marker("no comment"));
    }

    #[test]
    fn text_numbers_cover_separators_and_radix() {
        let nums = numbers_in_text("4 GiB shm, 50 MB, 1,600 lines, 0x10, 2.5 s, 1_000");
        for v in ["4", "50", "1600", "16", "2.5", "1000"] {
            assert!(nums.contains(v), "missing {v} in {nums:?}");
        }
    }

    #[test]
    fn explanation_reads_spec_description_and_linked_decisions() {
        let mut store = aida_core::RequirementsStore::default();
        let mut spec = aida_core::Requirement::new("Throttle".into(), "io-full 15".into());
        spec.spec_id = Some("STORY-1".into());
        let mut decision =
            aida_core::Requirement::new("Use 40% cpu".into(), "ceiling chosen in review".into());
        decision.req_type = aida_core::RequirementType::Decision;
        decision.spec_id = Some("ADR-1".into());
        let mut unlinked = aida_core::Requirement::new("Other".into(), "77".into());
        unlinked.req_type = aida_core::RequirementType::Decision;
        decision.relationships.push(aida_core::Relationship {
            rel_type: aida_core::RelationshipType::References,
            target_id: spec.id,
            created_at: None,
            created_by: None,
        });
        store.requirements = vec![spec, decision, unlinked];
        let nums = explaining_numbers(&store, "story-1");
        assert!(nums.contains("15"));
        assert!(nums.contains("40"));
        assert!(!nums.contains("77"));
        assert!(explaining_numbers(&store, "STORY-404").is_empty());
    }
}
