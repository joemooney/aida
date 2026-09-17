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
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct CriteriaReport {
    pub(crate) spec: String,
    pub(crate) criteria: Vec<CriterionRow>,
    pub(crate) untested: Vec<String>,
    pub(crate) unanchored: Vec<TracedTest>,
}

/// Build the acceptance-criteria trace report: parse `## Acceptance`, scan Rust
/// test functions for criterion-qualified trace markers, and compute both gap
/// classes.
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
    let tests = scan_rust_tests_for_criteria(project_root, spec)
        .with_context(|| format!("scanning Rust tests under {}", project_root.display()))?;
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
            CriterionRow { criterion, tests }
        })
        .collect();
    let untested = rows
        .iter()
        .filter(|row| row.tests.is_empty())
        .map(|row| row.criterion.id.clone())
        .collect();

    CriteriaReport {
        spec: spec.to_string(),
        criteria: rows,
        untested,
        unanchored: unanchored_by_key.into_values().collect(),
    }
}

pub(crate) fn parse_acceptance_criteria(spec: &str, description: &str) -> Vec<Criterion> {
    let Some(section) = acceptance_section(description) else {
        return Vec::new();
    };
    section
        .lines()
        .filter_map(|line| {
            criterion_text_from_line(line).map(|text| criterion_from_text(spec, text))
        })
        .collect()
}

fn acceptance_section(description: &str) -> Option<String> {
    let mut in_section = false;
    let mut out = String::new();
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
            out.push_str(line);
            out.push('\n');
        }
    }
    Some(out).filter(|s| !s.trim().is_empty())
}

fn criterion_text_from_line(line: &str) -> Option<&str> {
    let t = line.trim();
    let rest = t
        .strip_prefix("- [ ] ")
        .or_else(|| t.strip_prefix("- [x] "))
        .or_else(|| t.strip_prefix("- [X] "))
        .or_else(|| t.strip_prefix("- "))
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("+ "))
        .or_else(|| numbered_prefix(t))
        .unwrap_or(t);
    let rest = rest.trim();
    (!rest.is_empty()).then_some(rest)
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

pub(crate) fn scan_rust_tests_for_criteria(root: &Path, spec: &str) -> Result<Vec<TracedTest>> {
    let mut files = Vec::new();
    collect_rust_files(root, &mut files);
    let mut tests = Vec::new();
    for path in files {
        let content = std::fs::read_to_string(&path)?;
        tests.extend(scan_rust_test_file(root, &path, &content, spec));
    }
    Ok(tests)
}

fn collect_rust_files(root: &Path, out: &mut Vec<PathBuf>) {
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
            collect_rust_files(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

fn scan_rust_test_file(root: &Path, path: &Path, content: &str, spec: &str) -> Vec<TracedTest> {
    let mut out = Vec::new();
    let mut pending_test = false;
    let mut current: Option<(String, usize, usize, Vec<String>)> = None;
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = line.trim_start();
        if trimmed.starts_with("#[test]") || trimmed.starts_with("#[tokio::test") {
            pending_test = true;
        }
        if current.is_none() && pending_test {
            if let Some(name) = rust_fn_name(trimmed) {
                let depth = brace_delta(line).max(0) as usize;
                current = Some((name, line_no, depth, traces_in_line(line, spec)));
                pending_test = false;
                continue;
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
        if row.tests.is_empty() {
            println!("  {} no traced Rust tests", "untested".yellow());
        } else {
            for test in &row.tests {
                println!("  {} {}:{}", "test".green(), test.path, test.line);
            }
        }
    }
    println!();
    print_gap("Untested criteria", &report.untested);
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
}
