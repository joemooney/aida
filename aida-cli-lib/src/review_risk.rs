//! Versioned, advisory-only review risk. No gate consumes this assessment.
// trace:STORY-2698 | ai:codex
use std::path::Path;

const CLASSIFIER_VERSION: u32 = 1;
const LARGE_DIFF_LINES: u64 = 500;
const LARGE_DIFF_FILES: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Tier {
    Fasttrack,
    Standard,
    Strict,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RiskAssessment {
    pub classifier_version: u32,
    pub advisory_only: bool,
    pub repository: String,
    pub base: Option<String>,
    pub head: Option<String>,
    pub tier: Tier,
    pub reasons: Vec<String>,
}

// Never consult working-tree files, config-supplied commands, or incoming
// reviewer claims. Disabling rename detection exposes BOTH old and new paths
// as delete/add entries; raw mode evidence includes executable/symlink changes.
// trace:STORY-2698 | ai:codex
pub(crate) fn assess(repo: &Path, base: Option<&str>, head: Option<&str>) -> RiskAssessment {
    let mut result = RiskAssessment {
        classifier_version: CLASSIFIER_VERSION,
        advisory_only: true,
        repository: repo
            .canonicalize()
            .unwrap_or_else(|_| repo.to_path_buf())
            .display()
            .to_string(),
        base: base.map(str::to_owned),
        head: head.map(str::to_owned),
        tier: Tier::Strict,
        reasons: vec![],
    };
    let evidence = || -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        let (base, head) = (base?, head?);
        // Full immutable object IDs only: no revision expressions or options.
        if ![base, head]
            .iter()
            .all(|s| matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return None;
        }
        let mb = crate::gate::review_carryover::single_merge_base(repo, base, head).ok()?;
        let diff = |format| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args([
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-renames",
                    "--no-relative",
                    "--no-color",
                    "--ignore-submodules=none",
                    format,
                    "-z",
                    &mb,
                    head,
                    "--",
                ])
                .stdin(std::process::Stdio::null())
                .output()
                .ok()?;
            out.status.success().then_some(out.stdout)
        };
        Some((diff("--raw")?, diff("--numstat")?, diff("--patch")?))
    };
    if let Some((raw, stats, patch)) = evidence() {
        (result.tier, result.reasons) = classify(&raw, &stats);
        if possible_oracle_edit(&patch) {
            result.tier = Tier::Strict;
            result
                .reasons
                .push("possible inline test-oracle weakening".into());
        }
    } else {
        result
            .reasons
            .push("unavailable or unsupported base/head diff evidence".into());
    }
    result
}

// Inline test modules share production file names. Path classification alone
// cannot detect removed assertions or new ignores there. This deliberately
// over-classifies rather than claiming a semantic proof that tests are safe.
// trace:STORY-2698 | ai:codex
fn possible_oracle_edit(patch: &[u8]) -> bool {
    let Ok(patch) = std::str::from_utf8(patch) else {
        return true;
    };
    patch.lines().any(|line| {
        (line.starts_with('-')
            && !line.starts_with("---")
            && [
                "assert",
                "expect(",
                "panic!",
                "#[test",
                "#[tokio::test",
                "cfg(test)",
            ]
            .iter()
            .any(|marker| line.contains(marker)))
            || (line.starts_with('+')
                && !line.starts_with("+++")
                && ["#[ignore", "cfg(", "should_panic"]
                    .iter()
                    .any(|marker| line.contains(marker)))
    })
}

// Sensitive paths take precedence over docs/test/script extensions. Test edits
// that remove assertions may weaken an oracle: conservatively classify any
// removed test lines as STRICT until a reviewed weakening detector exists.
// trace:STORY-2698 | ai:codex
fn sensitive(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.split('/').any(|part| {
        matches!(
            part,
            "gate"
                | "containment"
                | "launch"
                | "vendors"
                | "schema"
                | "schemas"
                | "migrations"
                | "db"
                | "hooks"
                | ".githooks"
                | ".github"
                | ".gitlab"
                | ".cargo"
                | ".aida"
                | "templates"
                | ".agents"
                | ".claude"
                | ".codex"
                | "discipline"
        )
    }) || [
        "auth",
        "grant",
        "lease",
        "seat",
        "merge",
        "ship",
        "store",
        "phase_gate",
        "lifecycle",
        "launch",
        "headless",
        "sandbox",
        "permission",
        "config",
        "oracle",
        "review",
        "queue",
        "orchestrat",
        "drain",
        "agent",
        "policy",
        "trust",
        "cargo",
        "build.rs",
        "makefile",
        "dockerfile",
        "requirements",
        "package",
        "lock",
        "ci.",
        "ci-",
        "ci_",
        "pre-push",
        "pre-commit",
        "pyproject",
        "setup.py",
        "setup.cfg",
        "go.mod",
        "go.sum",
        "cmake",
        "meson",
        "justfile",
        "build.gradle",
        "pom.xml",
        ".gitmodules",
    ]
    .iter()
    .any(|word| p.contains(word))
}

fn test_path(path: &str) -> bool {
    path.split('/').any(|c| matches!(c, "tests" | "test"))
        || path.ends_with("_tests.rs")
        || path.contains("/test_")
}

fn low_path(path: &str) -> bool {
    path.starts_with("docs/")
        || path.ends_with(".md")
        || path.ends_with(".txt")
        || test_path(path)
        || path.starts_with("scripts/")
}

// trace:STORY-2698 | ai:codex
fn classify(raw: &[u8], stats: &[u8]) -> (Tier, Vec<String>) {
    let unknown = || {
        (
            Tier::Strict,
            vec!["unreadable or unsupported diff evidence".into()],
        )
    };
    let (Ok(raw), Ok(stats)) = (std::str::from_utf8(raw), std::str::from_utf8(stats)) else {
        return unknown();
    };
    let mut paths = Vec::new();
    let mut reasons = Vec::new();
    let mut fields = raw.split_terminator('\0');
    while let Some(header) = fields.next() {
        let Some(path) = fields.next() else {
            return unknown();
        };
        let parts: Vec<_> = header.split_whitespace().collect();
        if parts.len() != 5
            || !parts[0].starts_with(':')
            || path.is_empty()
            || !matches!(parts[4], "A" | "D" | "M" | "T")
        {
            return unknown();
        }
        let old = &parts[0][1..];
        let new = parts[1];
        if ![old, new]
            .iter()
            .all(|m| matches!(*m, "000000" | "100644" | "100755" | "120000" | "160000"))
        {
            return unknown();
        }
        if old != new && old != "000000" && new != "000000"
            || [old, new]
                .iter()
                .any(|m| matches!(*m, "100755" | "120000" | "160000"))
        {
            reasons.push(format!(
                "mode, executable, symlink or submodule evidence: {path}"
            ));
        }
        if sensitive(path) {
            reasons.push(format!("sensitive path: {path}"));
        }
        paths.push(path);
    }
    let mut lines = 0u64;
    let mut stat_paths = Vec::new();
    for row in stats.split_terminator('\0') {
        let mut cols = row.splitn(3, '\t');
        let (Some(add), Some(del), Some(path)) = (cols.next(), cols.next(), cols.next()) else {
            return unknown();
        };
        let (Ok(add), Ok(del)) = (add.parse::<u64>(), del.parse::<u64>()) else {
            return unknown();
        };
        lines = lines.saturating_add(add).saturating_add(del);
        if test_path(path) && del > 0 {
            reasons.push(format!("possible test-oracle weakening: {path}"));
        }
        stat_paths.push(path);
    }
    if paths != stat_paths || paths.is_empty() || !raw.ends_with('\0') || !stats.ends_with('\0') {
        return unknown();
    }
    if lines > LARGE_DIFF_LINES || paths.len() > LARGE_DIFF_FILES {
        reasons.push(format!("large diff: {} paths, {lines} changed lines (limits {LARGE_DIFF_FILES}/{LARGE_DIFF_LINES})", paths.len()));
    }
    if !reasons.is_empty() {
        return (Tier::Strict, reasons);
    }
    if paths.iter().all(|p| low_path(p)) {
        (Tier::Fasttrack, vec!["only documentation, additive tests or non-CI scripts; advisory, eligibility not granted".into()])
    } else {
        (
            Tier::Standard,
            vec!["ordinary code diff below size thresholds".into()],
        )
    }
}

// trace:STORY-2698 | ai:codex
pub(crate) fn stored_for_display(
    root: &Path,
    key: &str,
    source: &crate::gate::review_verdict::VerdictSource,
    verdict: &crate::gate::review_verdict::RecordedVerdict,
) -> Option<serde_json::Value> {
    use crate::gate::{ledger, review_verdict as rv};
    let root = ledger::store_root(root).ok()?;
    let event = match source {
        rv::VerdictSource::Ledger { file, seq } => {
            let key = file.file_stem()?.to_str()?;
            let events = match ledger::ChangeKey::parse(key) {
                Some(change) => ledger::store::read_file(&root, change),
                None => ledger::store::read_unbound(&root, key),
            };
            events.into_iter().find(|e| e.seq == *seq)?
        }
        // Synced receipts project as files. Only use local advisory evidence
        // for that exact verdict round, never a different/newer local review.
        rv::VerdictSource::File(_) => {
            let event = ledger::project::newest_verdict_event(&root, key)?;
            let local = rv::parse_ledger_record(event.verdict_record()?)?;
            if verdict.kind != local.kind
                || verdict.recorded_at.is_none()
                || verdict.recorded_at != local.recorded_at
                || verdict.reviewed_sha.is_none()
                || verdict.reviewed_sha != local.reviewed_sha
            {
                return None;
            }
            event
        }
    };
    event.risk
}

// trace:STORY-2698 | ai:codex
pub(crate) fn notice(risk: Option<&serde_json::Value>) -> Option<String> {
    let risk: RiskAssessment = serde_json::from_value(risk?.clone()).ok()?;
    let tier = match risk.tier {
        Tier::Fasttrack => "FASTTRACK",
        Tier::Standard => "STANDARD",
        Tier::Strict => "STRICT",
    };
    Some(format!(
        "risk: {tier} (advisory only; classifier v{}) — {}",
        risk.classifier_version,
        risk.reasons.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(paths: &[(&str, u64, u64)]) -> (Vec<u8>, Vec<u8>) {
        let mut raw = String::new();
        let mut stats = String::new();
        for (path, add, del) in paths {
            raw.push_str(&format!(":100644 100644 abc def M\0{path}\0"));
            stats.push_str(&format!("{add}\t{del}\t{path}\0"));
        }
        (raw.into_bytes(), stats.into_bytes())
    }

    #[test]
    fn review_risk_classifier_precedence_and_thresholds() {
        for (paths, expected) in [
            (vec![("docs/usage.md", 5, 1)], Tier::Fasttrack),
            (vec![("tests/new_case.rs", 5, 0)], Tier::Fasttrack),
            (vec![("scripts/format.py", 5, 0)], Tier::Fasttrack),
            (vec![("src/render.rs", 5, 1)], Tier::Standard),
            (
                vec![("docs/usage.md", 5, 0), ("src/render.rs", 1, 1)],
                Tier::Standard,
            ),
            (
                vec![("docs/usage.md", 5, 0), ("src/gate/check.rs", 1, 1)],
                Tier::Strict,
            ),
            (vec![("tests/case.rs", 1, 1)], Tier::Strict),
            (vec![("src/render.rs", 501, 0)], Tier::Strict),
        ] {
            let (raw, stats) = diff(&paths);
            assert_eq!(classify(&raw, &stats).0, expected, "{paths:?}");
        }
        for path in [
            "AGENTS.md",
            "aida-core/templates/skills/aida-review.md",
            "src/vendors/headless.rs",
            "src/phase_gates.rs",
            "Cargo.toml",
            "Cargo.lock",
            "build.rs",
            ".github/workflows/ci.yml",
            ".gitlab-ci.yml",
            "scripts/check-ci.py",
            "src/config.rs",
            "src/db/store.rs",
            "src/auth.rs",
            "src/pr_ship.rs",
            "src/launch.rs",
        ] {
            let (raw, stats) = diff(&[(path, 1, 0)]);
            assert_eq!(classify(&raw, &stats).0, Tier::Strict, "{path}");
        }
    }

    #[test]
    fn review_risk_inline_test_edits_are_conservative() {
        assert!(possible_oracle_edit(
            b"-    assert_eq!(actual, expected);\n"
        ));
        assert!(possible_oracle_edit(b"+#[ignore]\n"));
        assert!(!possible_oracle_edit(
            b"+    assert_eq!(actual, expected);\n"
        ));
    }

    #[test]
    fn review_risk_modes_rename_and_unknown_never_lower_tier() {
        let (raw, stats) = diff(&[("src/gate/check.rs", 0, 1), ("docs/check.md", 1, 0)]);
        assert_eq!(classify(&raw, &stats).0, Tier::Strict);
        for modes in [
            "100644 100755",
            "120000 120000",
            "000000 160000",
            "100644 100600",
        ] {
            let raw = format!(":{modes} abc def M\0docs/help.md\0");
            assert_eq!(
                classify(raw.as_bytes(), b"1\t0\tdocs/help.md\0").0,
                Tier::Strict
            );
        }
        for (raw, stats) in [
            (vec![], vec![]),
            (vec![255], vec![]),
            diff(&[("image.png", 0, 0)]),
        ] {
            let stats = if !raw.is_empty() && raw[0] != 255 {
                b"-\t-\timage.png\0".to_vec()
            } else {
                stats
            };
            assert_eq!(classify(&raw, &stats).0, Tier::Strict);
        }
        assert_eq!(
            assess(Path::new("/nonexistent"), None, None).tier,
            Tier::Strict
        );
    }
}
