//! `aida lint constants` — numeric literals in traced Rust code that the
//! owning spec never explains (TASK-1785, slice 2 of STORY-1672).
//!
//! Traced regions follow the existing trace conventions: a module-level
//! `//!` anchor covers the whole file (as in `aida trace coverage`), and a
//! marker inside an item or in its leading comment/blank prefix covers that
//! item (as in the mandate trace evidence), where items are functions,
//! `impl` blocks (so an impl-level marker covers its methods), consts and
//! statics. Every integer/float literal inside a traced region is checked
//! against the owning specs: it is explained when its normalized value
//! appears in a spec's description or in a decision linked to that spec.
//! Only 0, 1 and literals on a line carrying `// arbitrary` are exempt.
//! Report-only: nothing here writes to the store.
// trace:TASK-1785 | ai:claude

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

use anyhow::Result;
use serde::Serialize;
use syn::{spanned::Spanned, visit::Visit};

use crate::cli::LintConstantsFailOn;
use crate::{find_project_root, load_store_for_lookup};

/// Numbers in free text: radix-prefixed integers, or decimal digits with
/// optional `_`/`,` groups and a fraction. Boundaries are checked by hand.
static TEXT_NUMBER_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)0x[0-9a-f_]+|0o[0-7_]+|0b[01_]+|\d[\d_]*(?:,\d+)*(?:\.\d+)?").unwrap()
});

/// The `// arbitrary` / `// arbitrary: <reason>` silencing marker.
static ARBITRARY_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"//\s*arbitrary(?:\s*:|\s|$)").unwrap());

/// One numeric literal found inside a traced region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TracedLiteral {
    /// 1-based source line.
    pub(crate) line: usize,
    /// The literal as written (`0x0F`, `4_096u64`).
    pub(crate) literal: String,
    /// The normalized value it is compared by (`15`, `4096`).
    pub(crate) value: String,
    /// Owning spec ids from the file anchor and every enclosing traced item.
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
    /// Owning trace ids that resolve to no spec in the store: nothing under
    /// them can be explained.
    pub(crate) unresolved_specs: BTreeSet<String>,
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

/// Every number mentioned in free text, normalized the same way as literals.
/// A number glued to an identifier or id (`TASK-1785`, `sha256`, `#12`), or
/// part of a version, date or time (`v0.15.0`, `2026-10-15`, `08:38`) is not
/// a mention; a trailing unit (`50MB`, `15s`) is. A comma group reads as one
/// thousands-separated number (`1,600`, `100,000,000`) when every later group
/// has three digits and the first is shorter or a later group is `000`;
/// otherwise as a list (`100,200,300`). PURE.
pub(crate) fn numbers_in_text(text: &str) -> HashSet<String> {
    let bytes = text.as_bytes();
    let mut out = HashSet::new();
    for m in TEXT_NUMBER_RE.find_iter(text) {
        let before = m.start().checked_sub(1).map(|i| bytes[i]);
        if before.is_some_and(|b| b.is_ascii_alphanumeric() || b"-._:#".contains(&b)) {
            continue;
        }
        let after = bytes.get(m.end()).copied();
        let after_digit = bytes.get(m.end() + 1).is_some_and(u8::is_ascii_digit);
        if after.is_some_and(|b| b"-:.".contains(&b)) && after_digit {
            continue;
        }
        let raw = m.as_str().to_ascii_lowercase();
        let radix = [("0x", 16), ("0o", 8), ("0b", 2)]
            .into_iter()
            .find_map(|(p, r)| raw.strip_prefix(p).map(|d| (d.replace('_', ""), r)));
        if let Some((digits, radix)) = radix {
            out.extend(normalize_number(&digits, radix, false));
            continue;
        }
        let groups: Vec<&str> = raw.split(',').collect();
        let thousands = groups.len() == 1
            || (groups[1..]
                .iter()
                .all(|g| g.split('.').next().is_some_and(|d| d.len() == 3))
                && (groups[0].len() < 3 || groups[1..].iter().any(|g| g.starts_with("000"))));
        let parts = if thousands {
            vec![raw.replace(',', "")]
        } else {
            groups.iter().map(|g| g.to_string()).collect()
        };
        for part in parts {
            let digits = part.replace('_', "");
            out.extend(normalize_number(&digits, 10, digits.contains('.')));
        }
    }
    out
}

/// True when a line's comment text carries the `// arbitrary` marker
/// (optionally `// arbitrary: <reason>`). PURE.
fn is_arbitrary_marker(comment: &str) -> bool {
    ARBITRARY_RE.is_match(comment)
}

/// Line ranges (0-based, end-exclusive) of the items that can be traced
/// regions: functions, impl blocks, methods, trait default methods, consts
/// and statics. The second field ends the lines whose markers the item owns:
/// its whole span, except an impl owns only markers up to its `{` line, so
/// one method's marker never leaks to its siblings.
#[derive(Default)]
struct Regions(Vec<(std::ops::Range<usize>, usize)>);

impl Regions {
    fn push(&mut self, span: proc_macro2::Span) {
        self.0.push((
            span.start().line.saturating_sub(1)..span.end().line,
            span.end().line,
        ));
    }
}

impl<'ast> Visit<'ast> for Regions {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.push(node.span());
        syn::visit::visit_item_fn(self, node);
    }
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let span = node.span();
        self.0.push((
            span.start().line.saturating_sub(1)..span.end().line,
            node.brace_token.span.open().start().line,
        ));
        syn::visit::visit_item_impl(self, node);
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
    let ids = |c: &str| -> Vec<String> {
        anchor
            .captures_iter(c)
            .map(|cap| cap[1].to_ascii_uppercase())
            .collect()
    };

    // A module-level `//!` anchor covers every line of the file, the
    // FileAnchor rule of `aida trace coverage`.
    let file_specs: BTreeSet<String> = comments
        .iter()
        .zip(&lines)
        .filter(|(_, l)| l.trim_start().starts_with("//!"))
        .flat_map(|((_, c), _)| ids(c))
        .collect();

    let mut regions = Regions::default();
    regions.visit_file(&file);
    // Each item owns the markers in its span plus its leading comment/blank
    // prefix, attached exactly as the mandate trace evidence attaches it.
    let owned: Vec<(std::ops::Range<usize>, BTreeSet<String>)> = regions
        .0
        .into_iter()
        .filter_map(|(range, marker_end)| {
            let start =
                crate::mandate_traces::leading_comment_start(range.start, &lines, &comments);
            let specs: BTreeSet<String> = (start..marker_end)
                .filter_map(|i| comments.get(i))
                .flat_map(|(_, c)| ids(c))
                .collect();
            (!specs.is_empty()).then_some((range, specs))
        })
        .collect();
    if owned.is_empty() && file_specs.is_empty() {
        return Some(Vec::new());
    }

    let mut tokens = Vec::new();
    literal_tokens(text.parse().ok()?, &mut tokens);
    let mut out = Vec::new();
    for token in tokens {
        let line0 = token.span().start().line.saturating_sub(1);
        let mut specs = file_specs.clone();
        for (r, s) in &owned {
            if r.contains(&line0) {
                specs.extend(s.iter().cloned());
            }
        }
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

/// The numbers a spec explains: its description plus the title and
/// description of every decision linked to it in either direction. `None`
/// when no spec in the store carries this id.
fn explaining_numbers(
    store: &aida_core::RequirementsStore,
    spec_id: &str,
) -> Option<HashSet<String>> {
    let spec = store.requirements.iter().find(|r| {
        r.spec_id
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case(spec_id))
            || r.agreed_id
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(spec_id))
    })?;
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
    Some(numbers_in_text(&text))
}

/// Fold one file's traced literals into the report. `explains(spec, value)`
/// answers whether that spec explains the value, or `None` when the spec id
/// does not resolve. PURE (resolver injected).
pub(crate) fn tally_file<F>(
    report: &mut ConstantsReport,
    file: &str,
    literals: Vec<TracedLiteral>,
    explains: &mut F,
) where
    F: FnMut(&str, &str) -> Option<bool>,
{
    for lit in literals {
        report.literals_checked += 1;
        if lit.exempt {
            report.exempt += 1;
            continue;
        }
        let mut explained = false;
        for spec in &lit.specs {
            match explains(spec, &lit.value) {
                Some(true) => explained = true,
                Some(false) => {}
                None => {
                    report.unresolved_specs.insert(spec.clone());
                }
            }
        }
        if explained {
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

/// The literal as written, plus its normalized value when they differ.
fn display_literal(f: &ConstantFinding) -> String {
    if f.literal == f.value {
        f.literal.clone()
    } else {
        format!("{} (= {})", f.literal, f.value)
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

    let mut cache: HashMap<String, Option<HashSet<String>>> = HashMap::new();
    let mut explains = |id: &str, value: &str| {
        cache
            .entry(id.to_string())
            .or_insert_with(|| explaining_numbers(&store, id))
            .as_ref()
            .map(|nums| nums.contains(value))
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
                "unresolved_specs": report.unresolved_specs,
                "findings": report.findings,
            }))?
        );
    } else {
        for f in &report.findings {
            println!(
                "{}:{}  {}  {}",
                f.file,
                f.line,
                display_literal(f).yellow(),
                format!("(not in {})", f.specs.join(", ")).dimmed()
            );
        }
        for u in &report.unparsed {
            println!("{} {u}: did not parse, not checked", "skipped".dimmed());
        }
        if !report.unresolved_specs.is_empty() {
            let ids: Vec<&str> = report.unresolved_specs.iter().map(String::as_str).collect();
            println!(
                "{} trace id(s) not found in the store, so nothing under them can be explained: {}",
                "unresolved".yellow(),
                ids.join(", ")
            );
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

    fn explains_15(id: &str, value: &str) -> Option<bool> {
        (id == "STORY-1").then(|| numbers_in_text("Pause when io-full reaches 15.").contains(value))
    }

    fn values(src: &str) -> Vec<String> {
        scan_rust_source(src)
            .unwrap()
            .into_iter()
            .map(|l| l.value)
            .collect()
    }

    // The signed acceptance fixture: spec mentions 15; code has 15, 40 and a
    // marked 7 -> only 40 is flagged.
    // trace:TASK-1785.Fixture | ai:claude
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
        assert!(report.unresolved_specs.is_empty());
    }

    // trace:TASK-1785.ac070b6a | ai:claude
    #[test]
    fn untraced_code_is_not_scanned() {
        let src = "\
// trace:STORY-1 | ai:claude
fn traced() -> u32 { 40 }

fn untraced() -> u32 { 99 }

// an ordinary comment
const LIMIT: u32 = 98;
";
        assert_eq!(values(src), vec!["40"]);
    }

    // The leading prefix crosses blank lines, as in the mandate trace
    // evidence, and stops at code.
    // trace:TASK-1785.ac070b6a | ai:claude
    #[test]
    fn leading_marker_attaches_across_blank_lines() {
        let src = "\
// trace:STORY-1 | ai:claude

fn separated() -> u32 { 40 }
fn next() -> u32 { 41 }

/// Doc comment.
#[inline]
fn inside() -> u32 {
    // trace:STORY-2 | ai:claude
    42
}
";
        let lits = scan_rust_source(src).unwrap();
        let got: Vec<(&str, Vec<String>)> = lits
            .iter()
            .map(|l| (l.value.as_str(), l.specs.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("40", vec!["STORY-1".to_string()]),
                ("42", vec!["STORY-2".to_string()])
            ]
        );
    }

    // trace:TASK-1785.ac070b6a | ai:claude
    #[test]
    fn file_anchor_covers_the_whole_file() {
        let src = "\
//! Module docs. trace:STORY-9 | ai:claude

fn a() -> u32 { 40 }
const B: u32 = 41;
";
        let lits = scan_rust_source(src).unwrap();
        assert_eq!(lits.len(), 2);
        assert!(lits.iter().all(|l| l.specs == vec!["STORY-9".to_string()]));
    }

    // trace:TASK-1785.ac070b6a | ai:claude
    #[test]
    fn impl_marker_covers_its_methods() {
        let src = "\
// trace:STORY-1 | ai:claude
impl S {
    fn f(&self) -> u32 { 40 }
    // trace:STORY-2 | ai:claude
    fn g(&self) { println!(\"{}\", 50); }
}
";
        let lits = scan_rust_source(src).unwrap();
        assert_eq!(lits[0].specs, vec!["STORY-1".to_string()]);
        assert_eq!(lits[1].value, "50");
        assert_eq!(
            lits[1].specs,
            vec!["STORY-1".to_string(), "STORY-2".to_string()]
        );
    }

    // trace:TASK-1785.ac182306 | ai:claude
    #[test]
    fn literals_compare_by_normalized_value() {
        let src = "\
// trace:STORY-1 | ai:claude
const SIZES: [u64; 4] = [0x0F, 1_5u64, 15.0 as u64, 4_096];
";
        // `4` is the array length.
        assert_eq!(values(src), vec!["4", "15", "15", "15", "4096"]);
        let nums = numbers_in_text("width 4, rotate at 4,096 or 0xf");
        let mut report = ConstantsReport::default();
        let lits = scan_rust_source(src).unwrap();
        tally_file(&mut report, "f.rs", lits, &mut |_, v| {
            Some(nums.contains(v))
        });
        assert!(report.findings.is_empty(), "{:?}", report.findings);
    }

    // trace:TASK-1785.acd368eb | ai:claude
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

    // trace:TASK-1785.acd368eb | ai:claude
    #[test]
    fn arbitrary_marker_shapes() {
        assert!(is_arbitrary_marker("   // arbitrary"));
        assert!(is_arbitrary_marker("// arbitrary: picked by eye"));
        assert!(is_arbitrary_marker("//arbitrary"));
        assert!(!is_arbitrary_marker("// arbitrarily chosen"));
        assert!(!is_arbitrary_marker("no comment"));
    }

    // trace:TASK-1785.ac182306 | ai:claude
    #[test]
    fn text_numbers_cover_separators_units_and_radix() {
        let nums = numbers_in_text(
            "4 GiB shm, 50MB, 1,600 lines, 100,000,000 rows, 0x10, 2.5 s, 1_000, reaches 15.",
        );
        for v in ["4", "50", "1600", "100000000", "16", "2.5", "1000", "15"] {
            assert!(nums.contains(v), "missing {v} in {nums:?}");
        }
        assert!(!nums.contains("600"), "{nums:?}");
    }

    // Ids, hashes, versions, dates and times are not mentions; a list of
    // three-digit numbers is not one big number.
    // trace:TASK-1785.ac182306 | ai:claude
    #[test]
    fn text_numbers_skip_ids_versions_dates_and_lists() {
        let nums = numbers_in_text(
            "TASK-1785 STORY-1672 ADR-54 sha256 v0.15.0 #2723 2026-10-15 08:38 x86_64",
        );
        assert!(nums.is_empty(), "{nums:?}");
        let list = numbers_in_text("pick 100,200,300");
        let mut got: Vec<_> = list.into_iter().collect();
        got.sort();
        assert_eq!(got, vec!["100", "200", "300"]);
    }

    // trace:TASK-1785.ac070b6a | ai:claude
    #[test]
    fn unresolved_owning_specs_are_reported() {
        let src = "\
// trace:STORY-404 | ai:claude
fn f() -> u32 { 40 }
";
        let mut report = ConstantsReport::default();
        let lits = scan_rust_source(src).unwrap();
        tally_file(&mut report, "f.rs", lits, &mut explains_15);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(
            report.unresolved_specs.into_iter().collect::<Vec<_>>(),
            vec!["STORY-404".to_string()]
        );
    }

    // trace:TASK-1785.acbc344f | ai:claude
    #[test]
    fn human_line_names_the_normalized_value() {
        let finding = |literal: &str, value: &str| ConstantFinding {
            file: "f.rs".into(),
            line: 1,
            literal: literal.into(),
            value: value.into(),
            specs: vec![],
        };
        assert_eq!(display_literal(&finding("0x28", "40")), "0x28 (= 40)");
        assert_eq!(display_literal(&finding("40", "40")), "40");
    }

    // trace:TASK-1785.ac182306 | ai:claude
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
        let nums = explaining_numbers(&store, "story-1").unwrap();
        assert!(nums.contains("15"));
        assert!(nums.contains("40"));
        assert!(!nums.contains("77"));
        assert!(explaining_numbers(&store, "STORY-404").is_none());
    }
}
