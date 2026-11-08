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
        // STORY-1386: the pre-implementation red run, when a drain recorded one.
        // trace:STORY-1386 | ai:claude
        let red_root = crate::main_worktree_root_from(project_root);
        if let Some(record) = crate::criteria_red_run::load_record(&red_root, display) {
            println!("Red run before implementation ({}):", record.recorded_at);
            for line in crate::criteria_red_run::summary_lines(&record) {
                println!("  {line}");
            }
        }
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
            "sh" => scan_shell_test_file(root, &path, &content, spec),
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
    let mut braces = BraceScanner::for_path(path);
    for (idx, line) in content.lines().enumerate() {
        if idx + 1 < start_line {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        let delta = braces.delta(line);
        depth += delta;
        // A `{` only inside a literal or comment does not open the body.
        // trace:TASK-b6b1278890774ef993d2ebc327862470-016 | ai:claude
        if delta != 0 || braces.opened() {
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

pub(crate) fn collect_rust_files(root: &Path, out: &mut Vec<PathBuf>) {
    collect_files(root, out, &["rs"]);
}

fn collect_test_files(root: &Path, out: &mut Vec<PathBuf>) {
    collect_files(
        root,
        out,
        &["rs", "py", "js", "jsx", "ts", "tsx", "go", "sh"],
    );
    // A shell file is a test only by its `test_*.sh` name (TASK-2089).
    out.retain(|path| {
        path.extension().and_then(|value| value.to_str()) != Some("sh")
            || path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.starts_with("test_"))
    });
}

/// TASK-2089: a `tests/test_*.sh` script is a sequence of sections, each
/// opened by an `info "Test <label>: …"` / `echo "Test <label>: …"` line (the
/// repo's shell convention) or a `test_*()` function. A section is named
/// `Test <label>` (or the function name) and carries the markers in its
/// `#` comments: those directly above its header and those inside it, up to
/// the next header.
// trace:TASK-1955 | ai:claude (TASK-2089)
fn scan_shell_test_file(root: &Path, path: &Path, content: &str, spec: &str) -> Vec<TracedTest> {
    let mut out = Vec::new();
    let mut pending = Vec::new();
    let mut current: Option<(String, usize, Vec<String>)> = None;
    for (index, line) in content.lines().enumerate() {
        if let Some(name) = shell_test_name(line.trim_start()) {
            if let Some((name, start, traces)) = current.take() {
                push_traced_test(&mut out, root, path, name, start, traces);
            }
            let mut traces = std::mem::take(&mut pending);
            traces.extend(traces_in_line(line, spec));
            current = Some((name, index + 1, traces));
            continue;
        }
        let trimmed = line.trim_start();
        // A comment block directly above the next header is that header's;
        // the same block inside a section also counts for the section.
        if trimmed.starts_with('#') {
            pending.extend(traces_in_line(line, spec));
        } else {
            if let Some((_, _, traces)) = current.as_mut() {
                traces.append(&mut pending);
            }
            pending.clear();
        }
    }
    if let Some((name, start, mut traces)) = current.take() {
        traces.append(&mut pending);
        push_traced_test(&mut out, root, path, name, start, traces);
    }
    out
}

fn shell_test_name(trimmed: &str) -> Option<String> {
    for lead in ["info \"", "echo \""] {
        if let Some(rest) = trimmed.strip_prefix(lead) {
            // A section header is `Test <label>:` with a numbered label
            // (`0`, `1b`, `4b2`). A status line such as `Test failed: …` or
            // `Test dir: …` is not one. trace:TASK-1955 | ai:claude (TASK-2257)
            let rest = rest.strip_prefix("Test ")?;
            let label: String = rest
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric())
                .collect();
            let numbered = label.starts_with(|ch: char| ch.is_ascii_digit());
            return (numbered && rest[label.len()..].starts_with(':'))
                .then(|| format!("Test {label}"));
        }
    }
    let name = trimmed.strip_prefix("function ").unwrap_or(trimmed);
    let ident: String = name
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect();
    (ident.starts_with("test") && name[ident.len()..].trim_start().starts_with("()"))
        .then_some(ident)
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
    let mut current: Option<(String, usize, isize, Vec<String>)> = None;
    let mut braces = BraceScanner::c_like();
    for (index, line) in content.lines().enumerate() {
        let line_no = index + 1;
        let trimmed = line.trim_start();
        if current.is_none() {
            if let Some(name) = test_name(trimmed) {
                braces = BraceScanner::c_like();
                let delta = braces.delta(line);
                let mut traces = std::mem::take(&mut pending_traces);
                traces.extend(traces_in_line(line, spec));
                current = Some((name, line_no, delta, traces));
            } else {
                update_pending_traces(&mut pending_traces, line, comment_prefix, spec);
            }
        } else if let Some((_, _, depth, traces)) = current.as_mut() {
            traces.extend(traces_in_line(line, spec));
            *depth += braces.delta(line);
        }
        // The body has started once the scanner saw a code-level `{`.
        // trace:TASK-b6b1278890774ef993d2ebc327862470-016 | ai:claude
        if braces.opened() && current.as_ref().is_some_and(|(_, _, depth, _)| *depth <= 0) {
            let (name, start, _, traces) = current.take().expect("current test");
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
    let mut braces = BraceScanner::rust();
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = line.trim_start();
        if trimmed.starts_with("#[test]") || trimmed.starts_with("#[tokio::test") {
            pending_test = true;
        }
        if current.is_none() {
            if pending_test {
                if let Some(name) = rust_fn_name(trimmed) {
                    braces = BraceScanner::rust();
                    let depth = braces.delta(line).max(0) as usize;
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
            let next = (*depth as isize + braces.delta(line)).max(0) as usize;
            *depth = next;
            // A header line with no brace (e.g. a `where` clause) does not
            // close the test; only a code-level `{` starts the body.
            // trace:TASK-b6b1278890774ef993d2ebc327862470-022 | ai:claude
            if next == 0 && line_no > *start && braces.opened() {
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

/// Net `{`/`}` balance of one source line on its own (no carried literal
/// state); the scanners use [`BraceScanner`] so a literal spanning lines
/// stays a literal.
#[cfg(test)]
fn brace_delta(line: &str) -> isize {
    BraceScanner::rust().delta(line)
}

/// Literal or comment the scanner is inside at the end of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LiteralState {
    Code,
    /// A `"` or (outside Rust) backtick or `'` literal (escapes honored).
    Quoted(char),
    /// A Rust raw string closed by `"` plus this many `#`.
    Raw(usize),
    /// A `/* */` block comment this many levels deep (Rust nests them).
    Block(usize),
}

/// Net `{`/`}` balance of source lines fed in order, ignoring braces inside
/// string, byte, raw, backtick, and char literals and `//` / `/* */`
/// comments. Literal and block-comment state carries ACROSS lines, so a
/// fixture such as `r#"{` ... `}"#;` does not end the fn early. In Rust only
/// `"` and raw strings may span lines (a backtick is plain text there, e.g.
/// in a doc comment); in JS/TS/Go only backtick literals do, and `'` quotes
/// a string or rune.
// trace:TASK-b6b1278890774ef993d2ebc327862470-011 | ai:claude
// trace:TASK-b6b1278890774ef993d2ebc327862470-017 | ai:claude
struct BraceScanner {
    state: LiteralState,
    rust: bool,
    /// A code-level `{` has been seen since the scanner was created.
    opened: bool,
}

impl BraceScanner {
    fn rust() -> Self {
        Self {
            state: LiteralState::Code,
            rust: true,
            opened: false,
        }
    }

    fn c_like() -> Self {
        Self {
            state: LiteralState::Code,
            rust: false,
            opened: false,
        }
    }

    fn for_path(path: &Path) -> Self {
        if path.extension().and_then(|value| value.to_str()) == Some("rs") {
            Self::rust()
        } else {
            Self::c_like()
        }
    }

    fn delta(&mut self, line: &str) -> isize {
        let depth = self.line_delta(line);
        // Only backtick literals (and block comments) cross a line break
        // outside Rust.
        if !self.rust
            && !matches!(
                self.state,
                LiteralState::Quoted('`') | LiteralState::Block(_)
            )
        {
            self.state = LiteralState::Code;
        }
        depth
    }

    /// Whether any line fed so far had a `{` outside literals and comments.
    fn opened(&self) -> bool {
        self.opened
    }

    fn line_delta(&mut self, line: &str) -> isize {
        let chars: Vec<char> = line.chars().collect();
        let mut depth = 0;
        let mut i = match self.resume(&chars, 0) {
            Some(next) => next,
            None => return 0,
        };
        while i < chars.len() {
            let ch = chars[i];
            let before = |back: usize| i.checked_sub(back).and_then(|at| chars.get(at));
            // `r` starts a raw string only as its own token or right after a lone `b`.
            let raw_prefix =
                !is_ident(before(1)) || (before(1) == Some(&'b') && !is_ident(before(2)));
            match ch {
                '/' if chars.get(i + 1) == Some(&'/') => break,
                '/' if chars.get(i + 1) == Some(&'*') => {
                    self.state = LiteralState::Block(1);
                    match self.resume(&chars, i + 2) {
                        Some(next) => i = next - 1,
                        None => break,
                    }
                }
                '{' => {
                    depth += 1;
                    self.opened = true;
                }
                '}' => depth -= 1,
                '"' => {
                    self.state = LiteralState::Quoted(ch);
                    match self.resume(&chars, i + 1) {
                        Some(next) => i = next - 1,
                        None => break,
                    }
                }
                '`' | '\'' if !self.rust => {
                    self.state = LiteralState::Quoted(ch);
                    match self.resume(&chars, i + 1) {
                        Some(next) => i = next - 1,
                        None => break,
                    }
                }
                // Rust raw strings: r"..", r#".."#, br#".."#.
                'r' if self.rust && raw_prefix => {
                    let hashes = chars[i + 1..].iter().take_while(|c| **c == '#').count();
                    if chars.get(i + 1 + hashes) == Some(&'"') {
                        self.state = LiteralState::Raw(hashes);
                        match self.resume(&chars, i + 2 + hashes) {
                            Some(next) => i = next - 1,
                            None => break,
                        }
                    }
                }
                // Char literals ('{', '\''); a lifetime ('a) has no closing quote.
                '\'' if chars.get(i + 1) == Some(&'\\') => {
                    i = skip_quoted(&chars, i + 1, '\'').unwrap_or(chars.len());
                }
                '\'' if chars.get(i + 2) == Some(&'\'') => i += 2,
                _ => {}
            }
            i += 1;
        }
        depth
    }

    /// Continue the open literal from `start`: the index just past its close
    /// (state back to `Code`), or `None` when it runs past the end of the line.
    fn resume(&mut self, chars: &[char], start: usize) -> Option<usize> {
        let close = match self.state {
            LiteralState::Code => return Some(start),
            LiteralState::Quoted(quote) => skip_quoted(chars, start, quote)?,
            LiteralState::Raw(hashes) => skip_raw(chars, start, hashes)?,
            LiteralState::Block(open) => match skip_block(chars, start, open, self.rust) {
                Ok(close) => close,
                Err(open) => {
                    self.state = LiteralState::Block(open);
                    return None;
                }
            },
        };
        self.state = LiteralState::Code;
        Some(close + 1)
    }
}

fn is_ident(ch: Option<&char>) -> bool {
    ch.is_some_and(|c| c.is_alphanumeric() || *c == '_')
}

/// Index of the closing `quote` at or after `start` (honoring `\` escapes),
/// or `None` when the literal runs past the end of the line.
fn skip_quoted(chars: &[char], start: usize, quote: char) -> Option<usize> {
    let mut i = start;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            c if c == quote => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// Index of the `/` closing a block comment `open` levels deep (nested
/// `/*` count only in Rust), or `Err` with the depth still open at the end
/// of the line.
fn skip_block(chars: &[char], start: usize, mut open: usize, nested: bool) -> Result<usize, usize> {
    let mut i = start;
    while i < chars.len() {
        match (chars[i], chars.get(i + 1)) {
            ('*', Some('/')) => {
                open -= 1;
                if open == 0 {
                    return Ok(i + 1);
                }
                i += 1;
            }
            ('/', Some('*')) if nested => {
                open += 1;
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    Err(open)
}

/// Index of the last `#` closing a raw string (or its `"` when unhashed).
fn skip_raw(chars: &[char], start: usize, hashes: usize) -> Option<usize> {
    (start..chars.len())
        .find(|&i| {
            chars[i] == '"'
                && chars[i + 1..]
                    .iter()
                    .take(hashes)
                    .filter(|c| **c == '#')
                    .count()
                    == hashes
        })
        .map(|i| i + hashes)
}

fn traces_in_line(line: &str, spec: &str) -> Vec<String> {
    let mut traces = Vec::new();
    let mut search = line;
    while let Some(pos) = search.find("trace:") {
        let hit = &search[pos..];
        if let Some(id) = crate::parse_trace_id_token(hit) {
            // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude — the
            // spec part (before a `.criterion` suffix) matches numerically.
            let base = id.split_once('.').map_or(id.as_str(), |(b, _)| b);
            if aida_core::id_identity::same_id(base, spec) {
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

    // Braces inside literals and comments do not move the scanner's depth.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-011 | ai:claude
    #[test]
    fn brace_delta_ignores_braces_in_literals_and_comments() {
        assert_eq!(
            super::brace_delta(r#"fs::write(&p, b"{not json").unwrap();"#),
            0
        );
        assert_eq!(super::brace_delta(r#"let s = "}\"{"; {"#), 1);
        assert_eq!(super::brace_delta(r##"let s = r#"{"}"#; }"##), -1);
        assert_eq!(super::brace_delta(r##"let s = br#"{{"#;"##), 0);
        assert_eq!(super::brace_delta("match c { '{' => 1, '\\'' => 2 }"), 0);
        assert_eq!(super::brace_delta("fn f<'a>(x: &'a str) {"), 1);
        assert_eq!(
            super::BraceScanner::c_like().delta("const t = `${a}{`; // }"),
            0
        );
        assert_eq!(super::brace_delta("for r in rows {"), 1);
        assert_eq!(super::brace_delta("} else {"), 0);
    }

    // Literal state carries across lines: a brace inside a multi-line string
    // or raw string does not move the depth on its continuation lines.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-011 | ai:claude
    #[test]
    fn brace_scanner_carries_literal_state_across_lines() {
        let mut rust = super::BraceScanner::rust();
        let deltas: Vec<isize> = ["fn f() {", "    let j = r#\"{", "  \"a\": 1", "}\"#;", "}"]
            .iter()
            .map(|line| rust.delta(line))
            .collect();
        assert_eq!(deltas, vec![1, 0, 0, 0, -1]);
        let mut rust = super::BraceScanner::rust();
        let deltas: Vec<isize> = ["let s = \"{\\\"", "} \\\" {", "}\"; {"]
            .iter()
            .map(|line| rust.delta(line))
            .collect();
        assert_eq!(deltas, vec![0, 0, 1]);
        let mut js = super::BraceScanner::c_like();
        let deltas: Vec<isize> = ["const t = `{", "  ${x} }", "`; {", "let q = 'say \"{'; }"]
            .iter()
            .map(|line| js.delta(line))
            .collect();
        assert_eq!(deltas, vec![0, 0, 1, -1]);
    }

    fn scan_rust(src: &str) -> Vec<super::TracedTest> {
        super::scan_rust_test_file(
            std::path::Path::new("/r"),
            std::path::Path::new("/r/x.rs"),
            src,
            "T-1",
        )
    }

    // trace:TASK-b6b1278890774ef993d2ebc327862470-011 | ai:claude
    #[test]
    fn traced_test_after_multi_line_raw_string_fixture_keeps_its_trace() {
        let src = "#[test]\nfn fixture() {\n    let j = r#\"{\n  \"a\": 1\n}\"#;\n    // trace:T-1.AC1 | ai:claude\n    assert!(j.len() > 0);\n}\n";
        let tests = scan_rust(src);
        assert_eq!(tests.len(), 1, "{tests:?}");
        assert_eq!(tests[0].traces, vec!["T-1.AC1".to_string()]);
    }

    // trace:TASK-b6b1278890774ef993d2ebc327862470-011 | ai:claude
    #[test]
    fn traced_test_after_multi_line_string_with_braces_keeps_its_trace() {
        let src = "#[test]\nfn fixture() {\n    let j = \"{\n  \\\"a\\\": {1}\n}}\";\n    // trace:T-1.AC1 | ai:claude\n    assert!(true);\n}\n";
        let tests = scan_rust(src);
        assert_eq!(tests.len(), 1, "{tests:?}");
        assert_eq!(tests[0].traces, vec!["T-1.AC1".to_string()]);
    }

    // trace:TASK-b6b1278890774ef993d2ebc327862470-011 | ai:claude
    #[test]
    fn js_test_after_multi_line_template_literal_keeps_its_trace() {
        let src = "test('fixture', () => {\n  const t = `{\n  \"a\": ${x}\n}`;\n  // trace:T-1.AC1\n  expect(t).toBeTruthy();\n});\n\ntest('next', () => {\n  // trace:T-1.AC2\n});\n";
        let tests = super::scan_javascript_test_file(
            std::path::Path::new("/r"),
            std::path::Path::new("/r/x.test.ts"),
            src,
            "T-1",
        );
        let by_name: std::collections::BTreeMap<_, _> = tests
            .iter()
            .map(|t| (t.name.as_str(), t.traces.clone()))
            .collect();
        assert_eq!(
            by_name.get("fixture").cloned(),
            Some(vec!["T-1.AC1".to_string()])
        );
        assert_eq!(
            by_name.get("next").cloned(),
            Some(vec!["T-1.AC2".to_string()])
        );
    }

    // A backtick or unmatched quote inside a block comment does not turn
    // the rest of a Rust fn into a string; nested block comments close only
    // at the outermost `*/`.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-017 | ai:claude
    #[test]
    fn rust_brace_scanner_skips_block_comments_and_plain_backticks() {
        let mut rust = super::BraceScanner::rust();
        let deltas: Vec<isize> = [
            "fn f() {",
            "    /* a stray \" or ` {",
            "       /* nested { */ still a comment }",
            "    */ let x = 1; {",
            "    /// doc with `code` and a lone `",
            "    let y = 2; }",
            "}",
        ]
        .iter()
        .map(|line| rust.delta(line))
        .collect();
        assert_eq!(deltas, vec![1, 0, 0, 1, 0, -1, -1]);
        // A backtick in Rust code is plain text; it never opens a literal.
        let mut rust = super::BraceScanner::rust();
        assert_eq!(rust.delta("let s = '`'; {"), 1);
        assert_eq!(rust.delta("}"), -1);
        // JS block comments carry across lines but do not nest.
        let mut js = super::BraceScanner::c_like();
        let deltas: Vec<isize> = ["/* { /* {", "*/ {", "}"]
            .iter()
            .map(|line| js.delta(line))
            .collect();
        assert_eq!(deltas, vec![0, 1, -1]);
    }

    // trace:TASK-b6b1278890774ef993d2ebc327862470-017 | ai:claude
    #[test]
    fn traced_test_after_block_comment_with_stray_quote_keeps_its_trace() {
        let src = "#[test]\nfn commented() {\n    /* don't \" { */\n    // trace:T-1.AC1 | ai:claude\n    assert!(true);\n}\n\n#[test]\nfn next() {\n    // trace:T-1.AC2 | ai:claude\n}\n";
        let tests = scan_rust(src);
        let by_name: std::collections::BTreeMap<_, _> = tests
            .iter()
            .map(|t| (t.name.as_str(), t.traces.clone()))
            .collect();
        assert_eq!(
            by_name.get("commented").cloned(),
            Some(vec!["T-1.AC1".to_string()])
        );
        assert_eq!(
            by_name.get("next").cloned(),
            Some(vec!["T-1.AC2".to_string()])
        );
    }

    // A pre-body line whose only `{` sits in a string does not start the
    // body, so the fn is not cut before its real opening brace.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-016 | ai:claude
    #[test]
    fn extract_fn_source_ignores_brace_in_pre_body_string() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.rs");
        let src = "#[test]\n#[should_panic(expected = \"{oops\")]\nfn panics()\n{\n    let a = 1;\n    assert!(a == 2);\n}\nfn after() {}\n";
        std::fs::write(&path, src).unwrap();
        let body = super::extract_fn_source(&path, 2);
        assert!(body.contains("assert!(a == 2);"), "{body}");
        assert!(body.trim_end().ends_with('}'), "{body}");
        assert!(!body.contains("fn after"), "{body}");
    }

    // trace:TASK-b6b1278890774ef993d2ebc327862470-016 | ai:claude
    #[test]
    fn braced_test_with_brace_only_in_pre_body_string_keeps_its_whole_body() {
        let src =
            "test(\"{weird name\",\n  () =>\n{\n  // trace:T-1.AC1\n  expect(1).toBe(1);\n});\n";
        let tests = super::scan_javascript_test_file(
            std::path::Path::new("/r"),
            std::path::Path::new("/r/x.test.ts"),
            src,
            "T-1",
        );
        assert_eq!(tests.len(), 1, "{tests:?}");
        assert_eq!(tests[0].traces, vec!["T-1.AC1".to_string()]);
    }

    #[test]
    fn traced_test_with_brace_in_byte_literal_keeps_its_whole_body() {
        let src = "#[test]\nfn literal() {\n    write(b\"{not json\");\n    // trace:T-1.AC1 | ai:claude\n    assert!(true);\n}\n";
        let tests = super::scan_rust_test_file(
            std::path::Path::new("/r"),
            std::path::Path::new("/r/x.rs"),
            src,
            "T-1",
        );
        assert_eq!(tests.len(), 1, "{tests:?}");
        assert_eq!(tests[0].traces, vec!["T-1.AC1".to_string()]);
    }

    // trace:TASK-b6b1278890774ef993d2ebc327862470-022 | ai:claude
    #[test]
    fn traced_test_with_where_clause_header_keeps_its_whole_body() {
        let src = "#[test]\nfn generic<T>()\nwhere\n    T: Default,\n{\n    // trace:T-1.AC1 | ai:claude\n    assert!(true);\n}\n";
        let tests = super::scan_rust_test_file(
            std::path::Path::new("/r"),
            std::path::Path::new("/r/x.rs"),
            src,
            "T-1",
        );
        assert_eq!(tests.len(), 1, "{tests:?}");
        assert_eq!(tests[0].line, 2);
        assert_eq!(tests[0].traces, vec!["T-1.AC1".to_string()]);
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

    // TASK-2089: shell test sections carry the markers above and inside them.
    // trace:TASK-1955 | ai:claude (TASK-2089)
    #[test]
    fn shell_test_sections_carry_their_criterion_markers() {
        let root = Path::new("/repo");
        let path = root.join("tests/test_sample.sh");
        let content = "\
# trace:STORY-1.A9 | ai:claude
set -e
info \"Test 0: the fixture refuses\"
# trace:STORY-1.A1 | ai:claude
run_it

info \"Test 1b: dry run\"
# trace:STORY-1.A2 | ai:claude
# trace:STORY-1.A3 | ai:claude
other

# trace:STORY-1.A4 | ai:claude
echo \"Test 2c: echo form\"
done_it
info \"no marker here\"
test_helper() {
  # trace:STORY-1.A5 | ai:claude
  true
}
";
        let tests = scan_shell_test_file(root, &path, content, "STORY-1");
        let got: Vec<(&str, Vec<&str>)> = tests
            .iter()
            .map(|t| {
                (
                    t.name.as_str(),
                    t.traces.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("Test 0", vec!["STORY-1.A1"]),
                ("Test 1b", vec!["STORY-1.A2", "STORY-1.A3"]),
                ("Test 2c", vec!["STORY-1.A4"]),
                ("test_helper", vec!["STORY-1.A5"]),
            ]
        );
        assert_eq!(tests[0].path, "tests/test_sample.sh");
        assert_eq!(tests[0].line, 3);
    }

    // TASK-2257: a status line inside a section is not a section header, so
    // the markers after it stay with the section they sit in.
    // trace:TASK-1955 | ai:claude
    #[test]
    fn a_shell_status_line_does_not_open_a_section() {
        let root = Path::new("/repo");
        let path = root.join("tests/test_sample.sh");
        let content = "\
info \"Test 1: the real section\"
echo \"Test failed: expected 0, got 1\"
echo \"Test dir: $DIR\"
info \"Test passed.\"
# trace:STORY-1.A1 | ai:claude
run_it
";
        let tests = scan_shell_test_file(root, &path, content, "STORY-1");
        let got: Vec<(&str, Vec<&str>)> = tests
            .iter()
            .map(|t| {
                (
                    t.name.as_str(),
                    t.traces.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(got, vec![("Test 1", vec!["STORY-1.A1"])]);
        assert_eq!(shell_test_name("echo \"Test failed: x\""), None);
        assert_eq!(
            shell_test_name("echo \"Test 4b2: x\""),
            Some("Test 4b2".into())
        );
        assert_eq!(shell_test_name("info \"Test 3 without colon\""), None);
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

/// Report acceptance criteria traced by Rust, pytest, JS/TS, and Go tests,
/// or — with `coverage` (alias `gap`) in place of a spec — the project's
/// capture-coverage figures: trailer share, criteria share, and the
/// criterion-to-test share, each as count/total.
#[derive(clap::Parser, Debug)]
#[clap(name = "criteria")]
pub(crate) struct CriteriaArgs {
    /// Requirement ID (UUID or SPEC-ID) whose acceptance criteria to
    /// inspect, or `coverage` / `gap` for the project-wide report.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-015 | ai:claude
    #[clap(value_parser = crate::spec_arg::spec_id)]
    pub spec: String,

    /// Emit JSON instead of a human report.
    #[clap(long)]
    pub json: bool,

    /// Lookback window in days for the project-wide report (the all-time
    /// figures are always shown alongside). Ignored for a single spec.
    // trace:STORY-1487 | ai:claude
    #[clap(long, default_value_t = crate::criteria_coverage::DEFAULT_WINDOW_DAYS, value_name = "DAYS")]
    pub window_days: u64,
}

fn run_criteria(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <CriteriaArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::command_registry::Flow::Continue),
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                CriteriaArgs {
                    spec,
                    json,
                    window_days,
                } => {
                    let store = backend.load()?;
                    let project_root = find_project_root()?;
                    // trace:STORY-1487 | ai:claude
                    crate::criteria_coverage::dispatch_criteria(
                        &project_root,
                        &store,
                        spec,
                        *window_days,
                        *json,
                    )?;
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                CriteriaArgs {
                    spec,
                    json,
                    window_days,
                } => {
                    let store = storage.load()?;
                    let project_root = find_project_root()
                        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| ".".into()));
                    // `coverage` / `gap` is the project-wide report, never a spec id.
                    // trace:STORY-1487 | ai:claude
                    criteria_coverage::dispatch_criteria(
                        &project_root,
                        &store,
                        spec,
                        *window_days,
                        *json,
                    )?;
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static CRITERIA: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "criteria",
        help_after: "graph",
        command: <CriteriaArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Read,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Store(run_criteria),
    };
