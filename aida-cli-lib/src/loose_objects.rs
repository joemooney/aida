//! The `loose-objects` doctor category (TASK-2053 S5).
//!
//! A contained seat's clone borrows the main checkout's objects through
//! `--reference`, so creating one reads the whole object store. On
//! 2026-10-22 main held 128k loose objects and a cold seat clone took 35.7 s
//! (p95); after `git repack -d` left 7k it took 3.3 s. The store's lowered
//! `gc.auto` did not keep them down, so this scan reports a main checkout
//! whose loose-object count is past a configurable ceiling
//! (`[doctor.loose_objects] max`). Silent when healthy.
// trace:TASK-2053 | ai:claude

use std::path::Path;

use crate::DoctorFinding;

pub(crate) const CATEGORY: &str = "loose-objects";

/// Loose objects above which seat-clone creation slows noticeably.
const DEFAULT_MAX_LOOSE: usize = 20_000;

/// Read `[doctor.loose_objects] max`, falling back to the default ceiling.
fn max_loose(cfg: Option<&toml::Value>) -> usize {
    cfg.and_then(|c| c.get("doctor"))
        .and_then(|d| d.get("loose_objects"))
        .and_then(|d| d.get("max"))
        .and_then(|v| v.as_integer())
        .filter(|v| *v > 0)
        .map(|v| v as usize)
        .unwrap_or(DEFAULT_MAX_LOOSE)
}

/// The finding for `loose` objects against `max`; `None` when healthy.
fn finding(project_root: &Path, loose: usize, max: usize) -> Option<DoctorFinding> {
    (loose > max).then(|| DoctorFinding {
        category: CATEGORY.to_string(),
        id: CATEGORY.to_string(),
        summary: format!(
            "the main checkout's git object store holds {loose} loose objects (ceiling {max}); \
             every new contained seat's clone reads them all, so creating one is slow when \
             they are not in the page cache"
        ),
        action: format!(
            "pack them with `git -C {} repack -d` (it can take several minutes; nothing is \
             deleted), or raise `[doctor.loose_objects] max` deliberately",
            crate::shell_quote(&project_root.display().to_string())
        ),
        safe_heal: false,
    })
}

/// Scan the main checkout's object store.
pub(crate) fn scan(project_root: &Path) -> Vec<DoctorFinding> {
    let cfg = crate::read_project_config_value(project_root);
    let mut findings = Vec::new();
    // trace:TASK-2053 | ai:codex
    if let Some(value) = cfg
        .as_ref()
        .and_then(|c| c.get("doctor"))
        .and_then(|d| d.get("loose_objects"))
        .and_then(|d| d.get("max"))
    {
        if value.as_integer().is_none() {
            findings.push(DoctorFinding {
                category: CATEGORY.into(),
                id: "loose-objects-config".into(),
                summary: format!(
                    "[doctor.loose_objects] max must be an integer; using {DEFAULT_MAX_LOOSE}"
                ),
                action: "set `[doctor.loose_objects] max` to a positive integer".into(),
                safe_heal: false,
            });
        }
    }
    if let Some(finding) = aida_core::git_ops::count_objects(project_root)
        .and_then(|counts| finding(project_root, counts.loose, max_loose(cfg.as_ref())))
    {
        findings.push(finding);
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ceiling_defaults_and_reads_config() {
        assert_eq!(max_loose(None), DEFAULT_MAX_LOOSE);
        let cfg: toml::Value = toml::from_str("[doctor.loose_objects]\nmax = 500\n").unwrap();
        assert_eq!(max_loose(Some(&cfg)), 500);
        let zero: toml::Value = toml::from_str("[doctor.loose_objects]\nmax = 0\n").unwrap();
        assert_eq!(max_loose(Some(&zero)), DEFAULT_MAX_LOOSE);
    }

    #[test]
    fn only_a_count_past_the_ceiling_is_a_finding() {
        let root = Path::new("/repo");
        assert!(finding(root, 20_000, 20_000).is_none());
        let f = finding(root, 128_766, 20_000).expect("finding");
        assert_eq!(f.category, CATEGORY);
        assert!(f.summary.contains("128766 loose objects"), "{}", f.summary);
        assert!(f.action.contains("git -C /repo repack -d"), "{}", f.action);
        assert!(!f.safe_heal);
    }

    #[test]
    fn scan_reports_a_repo_past_the_ceiling() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        };
        git(&["init", "-q"]);
        for i in 0..3 {
            std::fs::write(repo.join(format!("f{i}")), format!("{i}\n")).unwrap();
        }
        git(&["-c", "gc.auto=0", "add", "."]);
        std::fs::create_dir_all(repo.join(".aida")).unwrap();
        std::fs::write(
            repo.join(".aida/config.toml"),
            "[doctor.loose_objects]\nmax = 2\n",
        )
        .unwrap();
        let findings = scan(repo);
        assert_eq!(findings.len(), 1, "{findings:#?}");
        std::fs::write(
            repo.join(".aida/config.toml"),
            "[doctor.loose_objects]\nmax = 100\n",
        )
        .unwrap();
        assert!(scan(repo).is_empty());
    }
}

#[cfg(test)]
mod review_followup_tests {
    use super::*;

    // trace:TASK-2198 | ai:codex
    #[test]
    fn remediation_round_trips_a_quoted_root_through_the_shell() {
        let root = Path::new("/tmp/Joe's repo $(false)");
        let finding = finding(root, 2, 1).unwrap();
        let command = finding.action.split('`').nth(1).unwrap();
        let args = command.strip_prefix("git ").unwrap();
        let output = std::process::Command::new("sh")
            .args(["-c", &format!("set -- {args}; printf '%s\\n' \"$@\"")])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "-C\n/tmp/Joe's repo $(false)\nrepack\n-d\n"
        );
    }

    // trace:TASK-2053 | ai:codex
    #[test]
    fn non_integer_max_reports_a_config_warning() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".aida")).unwrap();
        for value in ["\"many\"", "1.5", "true"] {
            std::fs::write(
                tmp.path().join(".aida/config.toml"),
                format!("[doctor.loose_objects]\nmax = {value}\n"),
            )
            .unwrap();
            let findings = scan(tmp.path());
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].id, "loose-objects-config");
            assert!(findings[0].summary.contains("must be an integer"));
            assert!(!findings[0].safe_heal);
        }
    }
}
