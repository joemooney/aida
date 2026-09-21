use anyhow::{Context, Result};
use colored::Colorize;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct Criterion {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct TracedTest {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) traces: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct CriterionRow {
    pub(crate) criterion: Criterion,
    pub(crate) tests: Vec<TracedTest>,
    pub(crate) state: CriterionState,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CriterionState {
    Traced,
    Untraced,
    PostDeployment,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct CriteriaReport {
    pub(crate) spec: String,
    pub(crate) criteria: Vec<CriterionRow>,
    pub(crate) untested: Vec<String>,
    pub(crate) post_deployment: Vec<String>,
    pub(crate) unanchored: Vec<TracedTest>,
}

/// Build the acceptance-criteria trace report: parse `## Acceptance`, scan
/// supported test files for criterion-qualified trace markers, and compute both
/// gap classes.
// trace:TASK-1246 | ai:codex
pub(crate) fn handle_criteria_command(
    project_root: &Path,
    store: &aida_core::RequirementsStore,
    spec: &str,
    json: bool,
) -> Result<()> {
    let req = store
        .get_requirement_by_spec_id(spec)
        .or_else(|| {
            store.requirements.iter().find(|r| {
                r.agreed_id
                    .as_deref()
                    .is_some_and(|id| id.eq_ignore_ascii_case(spec))
                    || r.id.to_string().eq_ignore_ascii_case(spec)
            })
        })
        .ok_or_else(|| anyhow::anyhow!("requirement not found: {spec}"))?;
    let display = req
        .spec_id
        .as_deref()
        .or(req.agreed_id.as_deref())
        .unwrap_or(spec);
    let report = build_criteria_report(project_root, display, &req.description)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human_report(&report);
    }
    Ok(())
}

pub(crate) fn build_criteria_report(
    project_root: &Path,
    spec: &str,
    description: &str,
) -> Result<CriteriaReport> {
    let criteria = parse_acceptance_criteria(spec, description);
    let tests = scan_tests_for_criteria(project_root, spec)
        .with_context(|| format!("scanning tests under {}", project_root.display()))?;
    Ok(criteria_report_from_parts(spec, criteria, tests))
}

fn criteria_report_from_parts(
    spec: &str,
    criteria: Vec<Criterion>,
    tests: Vec<TracedTest>,
) -> CriteriaReport {
    let known: BTreeSet<String> = criteria.iter().map(|c| c.id.to_ascii_uppercase()).collect();
    let mut tests_by_ac: BTreeMap<String, Vec<TracedTest>> = BTreeMap::new();
    let mut unanchored_by_key: BTreeMap<(String, usize, String), TracedTest> = BTreeMap::new();

    for test in tests {
        let mut anchored = false;
        for trace in &test.traces {
            let trace_upper = trace.to_ascii_uppercase();
            if trace_upper == spec.to_ascii_uppercase() {
                continue;
            }
            if known.contains(&trace_upper) {
                tests_by_ac
                    .entry(trace_upper)
                    .or_default()
                    .push(test.clone());
                anchored = true;
            } else if trace_upper.starts_with(&format!("{}.", spec.to_ascii_uppercase())) {
                unanchored_by_key
                    .entry((test.path.clone(), test.line, test.name.clone()))
                    .or_insert_with(|| test.clone());
            }
        }
        if !anchored
            && test
                .traces
                .iter()
                .any(|trace| trace.eq_ignore_ascii_case(spec))
        {
            unanchored_by_key
                .entry((test.path.clone(), test.line, test.name.clone()))
                .or_insert(test);
        }
    }

    let rows: Vec<CriterionRow> = criteria
        .into_iter()
        .map(|criterion| {
            let tests = tests_by_ac
                .remove(&criterion.id.to_ascii_uppercase())
                .unwrap_or_default();
            let state = if is_post_deployment_criterion(&criterion.text) {
                CriterionState::PostDeployment
            } else if tests.is_empty() {
                CriterionState::Untraced
            } else {
                CriterionState::Traced
            };
            CriterionRow {
                criterion,
                tests,
                state,
            }
        })
        .collect();
    let untested = rows
        .iter()
        .filter(|row| row.state == CriterionState::Untraced)
        .map(|row| row.criterion.id.clone())
        .collect();
    let post_deployment = rows
        .iter()
        .filter(|row| row.state == CriterionState::PostDeployment)
        .map(|row| row.criterion.id.clone())
        .collect();

    CriteriaReport {
        spec: spec.to_string(),
        criteria: rows,
        untested,
        post_deployment,
        unanchored: unanchored_by_key.into_values().collect(),
    }
}

/// Shallow, advisory detection of outcome criteria whose evidence can only
/// exist after the shipping change has been deployed. These are deliberately
/// excluded from the untraced/blocking set; a human should confirm and split
/// them onto a follow-up measurement spec. trace:TASK-1293 | ai:codex
pub(crate) fn is_post_deployment_criterion(text: &str) -> bool {
    let normalized = text
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let direct = [
        "measured on the next ",
        "measure on the next ",
        "over the following ",
        "after this ships",
        "after the change ships",
        "once deployed",
        "after deployment, measure ",
    ];
    if direct.iter().any(|phrase| normalized.contains(phrase)) {
        return true;
    }

    // Cover forms such as "after 20 specs" without pretending to understand
    // arbitrary prose. Requiring a numeric window keeps the heuristic honest.
    let words: Vec<&str> = normalized.split_whitespace().collect();
    words.windows(3).any(|window| {
        window[0] == "after"
            && window[1]
                .trim_matches(|c: char| !c.is_ascii_digit())
                .parse::<u64>()
                .is_ok()
            && matches!(
                window[2].trim_matches(|c: char| !c.is_alphanumeric()),
                "spec" | "specs" | "runs" | "deployments"
            )
    })
}

pub(crate) fn parse_acceptance_criteria(spec: &str, description: &str) -> Vec<Criterion> {
    // trace:BUG-1216 | ai:codex
    // trace:BUG-1219 | ai:codex
    // Parse both supported section forms. A malformed/migrating spec that has
    // both must not silently lose the inline criteria to heading precedence.
    [
        headed_acceptance_criteria(description),
        inline_acceptance_criteria(description),
    ]
    .into_iter()
    .flatten()
    .map(|text| criterion_from_text(spec, &text))
    .collect()
}

fn headed_acceptance_criteria(description: &str) -> Vec<String> {
    let mut in_section = false;
    let mut lines = Vec::new();
    for line in description.lines() {
        let trimmed = line.trim();
        if let Some(title) = trimmed.strip_prefix('#') {
            let title = title.trim_start_matches('#').trim();
            if in_section && !title.is_empty() {
                break;
            }
            if title.eq_ignore_ascii_case("acceptance")
                || title
                    .to_ascii_lowercase()
                    .starts_with("acceptance criteria")
            {
                in_section = true;
                continue;
            }
        }
        if in_section {
            lines.push(line);
        }
    }
    collect_bulleted_criteria(lines)
}

fn inline_acceptance_criteria(description: &str) -> Vec<String> {
    let lines: Vec<_> = description.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if is_inline_acceptance_marker(line) {
            return collect_bulleted_criteria(lines[index + 1..].iter().copied());
        }
    }
    Vec::new()
}

fn is_inline_acceptance_marker(line: &str) -> bool {
    let marker = line
        .trim()
        .trim_matches(|ch| matches!(ch, '*' | '_'))
        .trim();
    marker.eq_ignore_ascii_case("Acceptance:")
        || marker.eq_ignore_ascii_case("Acceptance criteria:")
}

fn collect_bulleted_criteria<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut criteria: Vec<String> = Vec::new();
    let mut saw_blank = false;
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with('#') {
            break;
        }
        if trimmed.is_empty() {
            saw_blank = true;
            continue;
        }
        if let Some(text) = bullet_text(trimmed) {
            criteria.push(text.to_string());
            saw_blank = false;
            continue;
        }
        if saw_blank {
            break;
        }
        if line.chars().next().is_some_and(char::is_whitespace) {
            if let Some(current) = criteria.last_mut() {
                current.push(' ');
                current.push_str(trimmed);
            }
        }
    }
    criteria
}

fn bullet_text(t: &str) -> Option<&str> {
    if explicit_acceptance_prefix(t).is_some() {
        return Some(t);
    }
    let rest = t
        .strip_prefix("- [ ] ")
        .or_else(|| t.strip_prefix("- [x] "))
        .or_else(|| t.strip_prefix("- [X] "))
        .or_else(|| t.strip_prefix("- "))
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| numbered_prefix(t))?;
    let rest = rest.trim();
    (!rest.is_empty()).then_some(rest)
}

fn explicit_acceptance_prefix(t: &str) -> Option<&str> {
    let upper = t.to_ascii_uppercase();
    let digits_start = if upper.starts_with("AC") {
        2
    } else if upper.starts_with('A') {
        1
    } else {
        return None;
    };
    let digits = upper[digits_start..]
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    if digits == 0 {
        return None;
    }
    let end = digits_start + digits;
    matches!(upper.as_bytes().get(end), Some(b'.' | b':')).then(|| t[end + 1..].trim_start())
}

fn numbered_prefix(t: &str) -> Option<&str> {
    let bytes = t.as_bytes();
    let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits > 0 && matches!(bytes.get(digits), Some(b'.' | b')')) {
        Some(t[digits + 1..].trim_start())
    } else {
        None
    }
}

fn criterion_from_text(spec: &str, text: &str) -> Criterion {
    let (label, body) = explicit_label(text).unwrap_or_else(|| {
        let label = format!("ac{}", stable_text_hash(text));
        (label, text.trim())
    });
    Criterion {
        id: format!("{spec}.{label}"),
        label,
        text: body.trim().to_string(),
    }
}

fn explicit_label(text: &str) -> Option<(String, &str)> {
    let trimmed = text.trim_start();
    let mut chars = trimmed.char_indices();
    let mut end = 0;
    let first = trimmed.chars().next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    for (idx, ch) in &mut chars {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            end = idx + ch.len_utf8();
            continue;
        }
        if matches!(ch, '.' | ':') && end > 0 {
            let label = trimmed[..end].to_string();
            let body = trimmed[idx + ch.len_utf8()..].trim_start();
            return (!body.is_empty()).then_some((label, body));
        }
        return None;
    }
    None
}

fn stable_text_hash(text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut hasher = Sha256::new();
    hasher.update(normalized.to_ascii_lowercase().as_bytes());
    let digest = hasher.finalize();
    format!("{:02x}{:02x}{:02x}", digest[0], digest[1], digest[2])
}

// Test discovery deliberately stays line-based: these scanners recognize the
// common declaration shapes without pretending to parse each language. All
// languages produce the same TracedTest and use the same adjacent-comment or
// in-body marker rule. trace:TASK-1258 | ai:codex
pub(crate) fn scan_tests_for_criteria(root: &Path, spec: &str) -> Result<Vec<TracedTest>> {
    let mut files = Vec::new();
    collect_test_files(root, &mut files);
    let mut tests = Vec::new();
    for path in files {
        let content = std::fs::read_to_string(&path)?;
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let found = match extension {
            "py" => scan_python_test_file(root, &path, &content, spec),
            "js" | "jsx" | "ts" | "tsx" => scan_javascript_test_file(root, &path, &content, spec),
            "go" => scan_go_test_file(root, &path, &content, spec),
            _ => scan_rust_test_file(root, &path, &content, spec),
        };
        tests.extend(found);
    }
    Ok(tests)
}

// TASK-1248: the source of one fn starting at `start_line` (1-based) in
/// `path`, cut by brace balance. Used to show the matching agent the real
/// traced test; empty when the file/line cannot be read.
// trace:TASK-1248 | ai:claude
pub(crate) fn extract_fn_source(path: &Path, start_line: usize) -> String {
    let Ok(content) = std::fs::read_to_string(path) else {
        return String::new();
    };
    if path.extension().and_then(|value| value.to_str()) == Some("py") {
        return extract_python_test_source(&content, start_line);
    }
    let mut out = String::new();
    let mut depth: isize = 0;
    let mut started = false;
    for (idx, line) in content.lines().enumerate() {
        if idx + 1 < start_line {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        depth += brace_delta(line);
        if brace_delta(line) != 0 || line.contains('{') {
            started = true;
        }
        if started && depth <= 0 {
            break;
        }
    }
    out
}

fn extract_python_test_source(content: &str, start_line: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let Some(first) = lines.get(start_line.saturating_sub(1)) else {
        return String::new();
    };
    let indent = first.len() - first.trim_start().len();
    let mut out = String::new();
    for (offset, line) in lines.iter().skip(start_line.saturating_sub(1)).enumerate() {
        let trimmed = line.trim_start();
        let line_indent = line.len() - trimmed.len();
        if offset > 0 && !trimmed.is_empty() && !trimmed.starts_with('#') && line_indent <= indent {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

// TASK-1248: names of the fns/structs/enums whose `trace:<spec>` comment sits
/// directly above them (non-test source only) — the "public surface" the
/// reconstitution probe is allowed to know about without seeing code.
// trace:TASK-1248 | ai:claude
pub(crate) fn symbols_traced_to_spec(root: &Path, spec: &str) -> Vec<String> {
    let mut files = Vec::new();
    collect_rust_files(root, &mut files);
    let needle = format!("trace:{}", spec.to_ascii_uppercase());
    let mut out = std::collections::BTreeSet::new();
    for path in files {
        if path.components().any(|c| c.as_os_str() == "tests") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let lines: Vec<&str> = content.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !line.to_ascii_uppercase().contains(&needle) {
                continue;
            }
            for cand in lines.iter().skip(i + 1).take(4) {
                let t = cand.trim_start();
                if t.starts_with("//") || t.starts_with("#[") {
                    continue;
                }
                if let Some(name) = item_name(t) {
                    out.insert(name);
                }
                break;
            }
        }
    }
    out.into_iter().collect()
}

fn item_name(trimmed: &str) -> Option<String> {
    let t = trimmed
        .trim_start_matches("pub(crate) ")
        .trim_start_matches("pub ")
        .trim_start_matches("async ");
    let (kind, rest) = if let Some(r) = t.strip_prefix("fn ") {
        ("fn", r)
    } else if let Some(r) = t.strip_prefix("struct ") {
        ("struct", r)
    } else if let Some(r) = t.strip_prefix("enum ") {
        ("enum", r)
    } else if let Some(r) = t.strip_prefix("const ") {
        ("const", r)
    } else {
        return None;
    };
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then(|| format!("{kind} {name}"))
}

fn collect_rust_files(root: &Path, out: &mut Vec<PathBuf>) {
    collect_files(root, out, &["rs"]);
}

fn collect_test_files(root: &Path, out: &mut Vec<PathBuf>) {
    collect_files(root, out, &["rs", "py", "js", "jsx", "ts", "tsx", "go"]);
}

fn collect_files(root: &Path, out: &mut Vec<PathBuf>, extensions: &[&str]) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if path.is_dir() {
            if matches!(
                name,
                ".git" | ".aida" | ".aida-store" | "target" | "node_modules" | "dist" | "build"
            ) {
                continue;
            }
            collect_files(&path, out, extensions);
        } else if path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|extension| extensions.contains(&extension))
        {
            out.push(path);
        }
    }
}

fn scan_python_test_file(root: &Path, path: &Path, content: &str, spec: &str) -> Vec<TracedTest> {
    let lines: Vec<&str> = content.lines().collect();
    let mut out = Vec::new();
    let mut pending_traces = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim_start();
        if let Some(name) = python_test_name(trimmed) {
            let indent = line.len() - trimmed.len();
            let start = index + 1;
            let mut traces = std::mem::take(&mut pending_traces);
            traces.extend(traces_in_line(line, spec));
            index += 1;
            while index < lines.len() {
                let body = lines[index];
                let body_trimmed = body.trim_start();
                let body_indent = body.len() - body_trimmed.len();
                if !body_trimmed.is_empty() && body_indent <= indent {
                    break;
                }
                traces.extend(traces_in_line(body, spec));
                index += 1;
            }
            push_traced_test(&mut out, root, path, name, start, traces);
            continue;
        }
        update_pending_traces(&mut pending_traces, line, "#", spec);
        index += 1;
    }
    out
}

fn python_test_name(trimmed: &str) -> Option<String> {
    let rest = trimmed
        .strip_prefix("def ")
        .or_else(|| trimmed.strip_prefix("async def "))?;
    let name: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect();
    name.starts_with("test_").then_some(name)
}

fn scan_javascript_test_file(
    root: &Path,
    path: &Path,
    content: &str,
    spec: &str,
) -> Vec<TracedTest> {
    scan_braced_test_file(root, path, content, spec, "//", javascript_test_name)
}

fn javascript_test_name(trimmed: &str) -> Option<String> {
    let rest = trimmed
        .strip_prefix("test(")
        .or_else(|| trimmed.strip_prefix("it("))?;
    let quote = rest.chars().next()?;
    if !matches!(quote, '\'' | '"' | '`') {
        return None;
    }
    let name = rest[quote.len_utf8()..].split(quote).next()?.to_string();
    (!name.is_empty()).then_some(name)
}

fn scan_go_test_file(root: &Path, path: &Path, content: &str, spec: &str) -> Vec<TracedTest> {
    scan_braced_test_file(root, path, content, spec, "//", go_test_name)
}

fn go_test_name(trimmed: &str) -> Option<String> {
    let rest = trimmed.strip_prefix("func Test")?;
    let suffix: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect();
    (!suffix.is_empty() && rest[suffix.len()..].trim_start().starts_with('('))
        .then(|| format!("Test{suffix}"))
}

fn scan_braced_test_file(
    root: &Path,
    path: &Path,
    content: &str,
    spec: &str,
    comment_prefix: &str,
    test_name: fn(&str) -> Option<String>,
) -> Vec<TracedTest> {
    let mut out = Vec::new();
    let mut pending_traces = Vec::new();
    let mut current: Option<(String, usize, isize, Vec<String>, bool)> = None;
    for (index, line) in content.lines().enumerate() {
        let line_no = index + 1;
        let trimmed = line.trim_start();
        if current.is_none() {
            if let Some(name) = test_name(trimmed) {
                let delta = brace_delta(line);
                let mut traces = std::mem::take(&mut pending_traces);
                traces.extend(traces_in_line(line, spec));
                current = Some((name, line_no, delta, traces, line.contains('{')));
            } else {
                update_pending_traces(&mut pending_traces, line, comment_prefix, spec);
            }
        } else if let Some((_, _, depth, traces, started)) = current.as_mut() {
            traces.extend(traces_in_line(line, spec));
            *depth += brace_delta(line);
            *started |= line.contains('{');
        }
        if current
            .as_ref()
            .is_some_and(|(_, _, depth, _, started)| *started && *depth <= 0)
        {
            let (name, start, _, traces, _) = current.take().expect("current test");
            push_traced_test(&mut out, root, path, name, start, traces);
        }
    }
    out
}

fn update_pending_traces(pending: &mut Vec<String>, line: &str, comment_prefix: &str, spec: &str) {
    let trimmed = line.trim_start();
    if trimmed.starts_with(comment_prefix) {
        pending.extend(traces_in_line(line, spec));
    } else {
        pending.clear();
    }
}

fn push_traced_test(
    out: &mut Vec<TracedTest>,
    root: &Path,
    path: &Path,
    name: String,
    line: usize,
    traces: Vec<String>,
) {
    let traces = dedupe(traces);
    if !traces.is_empty() {
        out.push(TracedTest {
            name,
            path: rel_path(root, path),
            line,
            traces,
        });
    }
}

fn scan_rust_test_file(root: &Path, path: &Path, content: &str, spec: &str) -> Vec<TracedTest> {
    let mut out = Vec::new();
    let mut pending_test = false;
    // BUG-1189: criterion markers on the comment/attribute lines immediately
    // ABOVE a test fn belong to that fn (the repo convention puts `// trace:`
    // above the item). A blank line or non-comment code between them breaks
    // the attachment so a stray marker never leaks onto the next fn.
    // trace:BUG-1189 | ai:claude
    let mut pending_traces: Vec<String> = Vec::new();
    let mut current: Option<(String, usize, usize, Vec<String>)> = None;
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = line.trim_start();
        if trimmed.starts_with("#[test]") || trimmed.starts_with("#[tokio::test") {
            pending_test = true;
        }
        if current.is_none() {
            if pending_test {
                if let Some(name) = rust_fn_name(trimmed) {
                    let depth = brace_delta(line).max(0) as usize;
                    let mut traces = std::mem::take(&mut pending_traces);
                    traces.extend(traces_in_line(line, spec));
                    current = Some((name, line_no, depth, traces));
                    pending_test = false;
                    continue;
                }
            }
            if trimmed.starts_with("//") || trimmed.starts_with("#[") {
                pending_traces.extend(traces_in_line(line, spec));
            } else if !pending_test {
                pending_traces.clear();
            }
        }
        if let Some((name, start, depth, traces)) = current.as_mut() {
            traces.extend(traces_in_line(line, spec));
            let next = (*depth as isize + brace_delta(line)).max(0) as usize;
            *depth = next;
            if next == 0 && line_no > *start {
                let traces = dedupe(std::mem::take(traces));
                if !traces.is_empty() {
                    out.push(TracedTest {
                        name: name.clone(),
                        path: rel_path(root, path),
                        line: *start,
                        traces,
                    });
                }
                current = None;
            }
        }
    }
    out
}

fn rust_fn_name(trimmed: &str) -> Option<String> {
    let rest = trimmed
        .strip_prefix("fn ")
        .or_else(|| trimmed.strip_prefix("pub fn "))
        .or_else(|| trimmed.strip_prefix("async fn "))
        .or_else(|| trimmed.strip_prefix("pub async fn "))?;
    let name = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect::<String>();
    (!name.is_empty()).then_some(name)
}

fn brace_delta(line: &str) -> isize {
    line.chars().fold(0, |acc, ch| match ch {
        '{' => acc + 1,
        '}' => acc - 1,
        _ => acc,
    })
}

fn traces_in_line(line: &str, spec: &str) -> Vec<String> {
    let mut traces = Vec::new();
    let mut search = line;
    while let Some(pos) = search.find("trace:") {
        let hit = &search[pos..];
        if let Some(id) = crate::parse_trace_id_token(hit) {
            if id.eq_ignore_ascii_case(spec)
                || id.starts_with(&format!("{}.", spec.to_ascii_uppercase()))
            {
                traces.push(id);
            }
        }
        search = &hit["trace:".len()..];
    }
    traces
}

fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect()
}

fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn print_human_report(report: &CriteriaReport) {
    println!("Acceptance criteria for {}", report.spec.bold());
    if report.criteria.is_empty() {
        println!("  no acceptance criteria found");
    }
    for row in &report.criteria {
        println!();
        println!("{} {}", row.criterion.id.cyan().bold(), row.criterion.text);
        if row.state == CriterionState::PostDeployment {
            println!(
                "  {} cannot be traced before this change ships; move the outcome, measurement window, and falsifying threshold to a follow-up measurement spec blocked by {}",
                "post-deployment".yellow(), report.spec
            );
        } else if row.tests.is_empty() {
            println!("  {} no traced tests", "untraced".yellow());
        } else {
            for test in &row.tests {
                println!("  {} {}:{}", "test".green(), test.path, test.line);
            }
        }
    }
    println!();
    print_gap("Untested criteria", &report.untested);
    print_gap(
        "Post-deployment criteria (advisory; do not block done)",
        &report.post_deployment,
    );
    if report.unanchored.is_empty() {
        println!("Unanchored tests: none");
    } else {
        println!("Unanchored tests:");
        for test in &report.unanchored {
            println!("  {}:{} {}", test.path, test.line, test.name);
        }
    }
}

fn print_gap(label: &str, ids: &[String]) {
    if ids.is_empty() {
        println!("{label}: none");
    } else {
        println!("{label}:");
        for id in ids {
            println!("  {id}");
        }
    }
}

#[cfg(test)]
mod tests {
    // BUG-1189: a criterion marker on the comment line ABOVE #[test] attaches to
    // that test (the repo convention); in-body markers keep working; a marker
    // separated by unrelated code does not leak onto the next fn.
    #[test]
    fn trace_above_test_attribute_attributes_to_that_test() {
        let src = "// trace:T-1.AC1 | ai:claude\n#[test]\nfn above() {\n    assert!(true);\n}\n\n#[test]\nfn inside() {\n    // trace:T-1.AC2 | ai:claude\n    assert!(true);\n}\n\n// trace:T-1.AC3 | ai:claude\nfn helper() {}\n\n#[test]\nfn unrelated() {\n    assert!(true);\n}\n";
        let tests = super::scan_rust_test_file(
            std::path::Path::new("/r"),
            std::path::Path::new("/r/x.rs"),
            src,
            "T-1",
        );
        let by_name: std::collections::BTreeMap<_, _> = tests
            .iter()
            .map(|t| (t.name.as_str(), t.traces.clone()))
            .collect();
        assert_eq!(
            by_name.get("above").cloned(),
            Some(vec!["T-1.AC1".to_string()])
        );
        assert_eq!(
            by_name.get("inside").cloned(),
            Some(vec!["T-1.AC2".to_string()])
        );
        assert!(by_name.get("unrelated").is_none(), "{by_name:?}");
    }

    use super::*;

    #[test]
    fn labeled_and_unlabeled_criteria_get_stable_ids() {
        let desc = "Intro\n\n## Acceptance\n\n- A1. First thing\n- unlabeled thing\n";
        let first = parse_acceptance_criteria("STORY-1", desc);
        let reordered = parse_acceptance_criteria(
            "STORY-1",
            "## Acceptance\n- unlabeled thing\n- A1. First thing\n",
        );
        assert_eq!(first[0].id, "STORY-1.A1");
        assert_eq!(first[0].text, "First thing");
        assert!(first[1].id.starts_with("STORY-1.ac"));
        assert_eq!(first[1].id, reordered[0].id);
        assert_eq!(first[0].id, reordered[1].id);
    }

    #[test]
    fn inline_acceptance_marker_must_be_standalone_and_only_collects_bullets() {
        // trace:BUG-1219 | ai:codex
        let desc = "Context with inline Acceptance: prose.\n/home/joe/ai/aida-task-1\nafter a refused rebase";
        let criteria = parse_acceptance_criteria("STORY-1", desc);
        assert!(criteria.is_empty());

        let desc = "**Acceptance:**\n- first outcome\n2) second outcome\nAC3: third outcome";
        let criteria = parse_acceptance_criteria("STORY-1", desc);
        assert_eq!(criteria.len(), 3);
        assert_eq!(criteria[0].text, "first outcome");
        assert_eq!(criteria[1].text, "second outcome");
        assert_eq!(criteria[2].text, "third outcome");
        assert_eq!(criteria[2].label, "AC3");
    }

    #[test]
    fn acceptance_sections_join_continuations_and_stop_at_fences_or_prose() {
        // trace:BUG-1219 | ai:codex
        let desc = "Acceptance criteria:\n- first line\n  continued line\n\nprose after section\n- not included";
        let criteria = parse_acceptance_criteria("BUG-1", desc);
        assert_eq!(criteria.len(), 1);
        assert_eq!(criteria[0].text, "first line continued line");

        let desc = "## Acceptance\n- before fence\n```text\n- inside fence\n```\n- after fence";
        let criteria = parse_acceptance_criteria("BUG-1", desc);
        assert_eq!(criteria.len(), 1);
        assert_eq!(criteria[0].text, "before fence");
    }

    #[test]
    fn heading_and_inline_acceptance_sections_are_both_preserved() {
        let desc = "Acceptance:\n- original\n\n## Acceptance\n- AC1. harvested\n";
        let criteria = parse_acceptance_criteria("BUG-1", desc);
        assert_eq!(criteria.len(), 2);
        assert_eq!(criteria[0].text, "harvested");
        assert_eq!(criteria[1].text, "original");
    }

    #[test]
    fn gap_report_flags_untested_ac_and_unanchored_test() {
        let criteria = vec![
            Criterion {
                id: "STORY-1.A1".into(),
                label: "A1".into(),
                text: "covered".into(),
            },
            Criterion {
                id: "STORY-1.A2".into(),
                label: "A2".into(),
                text: "missing".into(),
            },
        ];
        let tests = vec![
            TracedTest {
                name: "covers_a1".into(),
                path: "tests/a.rs".into(),
                line: 3,
                traces: vec!["STORY-1.A1".into()],
            },
            TracedTest {
                name: "unknown_ac".into(),
                path: "tests/a.rs".into(),
                line: 9,
                traces: vec!["STORY-1.A9".into()],
            },
        ];
        let report = criteria_report_from_parts("STORY-1", criteria, tests);
        assert_eq!(report.untested, vec!["STORY-1.A2"]);
        assert_eq!(report.unanchored[0].name, "unknown_ac");
        assert_eq!(report.criteria[0].tests[0].name, "covers_a1");
        assert_eq!(report.criteria[0].state, CriterionState::Traced);
        assert_eq!(report.criteria[1].state, CriterionState::Untraced);
        assert!(report.post_deployment.is_empty());
    }

    #[test]
    fn post_deployment_criteria_are_advisory_not_untraced() {
        // trace:TASK-1293.ac1c7c47 | ai:codex
        let criteria = vec![
            Criterion {
                id: "TASK-1.A1".into(),
                label: "A1".into(),
                text: "Measured on the next 20 specs: median rounds-to-merge drops".into(),
            },
            Criterion {
                id: "TASK-1.A2".into(),
                label: "A2".into(),
                text: "After 12 runs the failure rate remains below 2%".into(),
            },
            Criterion {
                id: "TASK-1.A3".into(),
                label: "A3".into(),
                text: "The fixture reports a median value".into(),
            },
        ];

        let report = criteria_report_from_parts("TASK-1", criteria, vec![]);
        assert_eq!(report.post_deployment, vec!["TASK-1.A1", "TASK-1.A2"]);
        assert_eq!(report.untested, vec!["TASK-1.A3"]);
        assert_eq!(report.criteria[0].state, CriterionState::PostDeployment);
    }

    #[test]
    fn post_deployment_phrase_detection_is_shallow_and_case_insensitive() {
        // trace:TASK-1293.ac1c7c47 | ai:codex
        for text in [
            "Once deployed, the error rate falls",
            "OVER THE FOLLOWING ten releases, adoption rises",
            "After this ships, measure latency",
            "After 20 specs the median falls",
        ] {
            assert!(is_post_deployment_criterion(text), "missed: {text}");
        }
        assert!(!is_post_deployment_criterion(
            "The fixture exposes the median and rate"
        ));
        for text in [
            "The gate distinguishes post-deployment criteria from untraced criteria",
            "A post-deployment criterion is reported with suggested split guidance",
            "The report explains that post-deployment evidence cannot exist at merge time",
        ] {
            assert!(
                !is_post_deployment_criterion(text),
                "false positive: {text}"
            );
        }
    }

    #[test]
    fn discipline_pack_documents_shipping_split_with_task_1291_example() {
        // trace:TASK-1293.acb68aa3 | ai:codex
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../.aida/discipline/spec-authoring.md");
        let guidance = std::fs::read_to_string(path).unwrap();
        let normalized = guidance.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(normalized.contains("TASK-1291 originally required"));
        assert!(normalized.contains("follow-up measurement spec"));
        assert!(normalized.contains("blocked by the shipping spec"));
        assert!(normalized.contains("threshold that would falsify the change"));
    }

    #[test]
    fn rust_test_scan_finds_traced_tests_only() {
        let root = Path::new("/repo");
        let path = Path::new("/repo/tests/merge_lock.rs");
        let src = r#"
            #[test]
            fn traced() {
                // trace:STORY-1.A1 | ai:codex
                assert!(true);
            }

            #[test]
            fn bare_trace() {
                // trace:STORY-1 | ai:codex
                assert!(true);
            }
        "#;
        let tests = scan_rust_test_file(root, path, src, "STORY-1");
        assert_eq!(tests.len(), 2);
        assert_eq!(tests[0].name, "traced");
        assert_eq!(tests[0].traces, vec!["STORY-1.A1"]);
        assert_eq!(tests[1].traces, vec!["STORY-1"]);
    }

    #[test]
    fn language_fixtures_share_trace_attachment_rules() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let fixture_root = root.join("src/tests/fixtures/criteria");
        let cases = [
            ("sample.rs", vec!["rust_above", "rust_inside"]),
            (
                "sample.py",
                vec![
                    "test_python_above",
                    "test_python_inside",
                    "test_python_after_direct_marker",
                ],
            ),
            ("sample.ts", vec!["typescript above", "typescript inside"]),
            ("sample.go", vec!["TestGoAbove", "TestGoInside"]),
        ];
        for (file, expected) in cases {
            let path = fixture_root.join(file);
            let content = std::fs::read_to_string(&path).expect("fixture");
            let tests = match path.extension().and_then(|value| value.to_str()) {
                Some("py") => scan_python_test_file(root, &path, &content, "STORY-1"),
                Some("ts") => scan_javascript_test_file(root, &path, &content, "STORY-1"),
                Some("go") => scan_go_test_file(root, &path, &content, "STORY-1"),
                _ => scan_rust_test_file(root, &path, &content, "STORY-1"),
            };
            assert_eq!(
                tests
                    .iter()
                    .map(|test| test.name.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(tests[0].traces, vec!["STORY-1.A1"]);
            assert_eq!(tests[1].traces, vec!["STORY-1.A2"]);
        }
    }

    #[test]
    fn python_outdented_marker_attaches_to_following_test() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let path = root.join("src/tests/fixtures/criteria/sample.py");
        let content = std::fs::read_to_string(&path).expect("fixture");
        let tests = scan_python_test_file(root, &path, &content, "STORY-1");

        assert!(!tests
            .iter()
            .any(|test| test.name == "test_python_before_direct_marker"));
        let following = tests
            .iter()
            .find(|test| test.name == "test_python_after_direct_marker")
            .expect("directly-above marker belongs to the following test");
        assert_eq!(following.traces, vec!["STORY-1.A3"]);
    }

    #[test]
    fn mixed_language_tree_reports_paths_and_lines() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tests/fixtures/criteria");
        let tests = scan_tests_for_criteria(&root, "STORY-1").expect("scan fixtures");
        assert_eq!(tests.len(), 9);
        for test in tests {
            assert!(test.path.starts_with("sample."), "{}", test.path);
            assert!(test.line > 0);
        }
    }

    #[test]
    fn extracts_only_the_traced_python_test_body() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tests/fixtures/criteria");
        let source = extract_fn_source(&root.join("sample.py"), 6);
        assert!(source.contains("def test_python_inside"));
        assert!(source.contains("trace:STORY-1.A2"));
        assert!(!source.contains("test_python_untraced"));
    }
}
