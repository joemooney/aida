// trace:BUG-2690 | ai:codex
use super::{check_name, check_text, newest_checks, short, summarize_github_rollup, Ci, Pr};
use crate::forge::{landing_ci_api, CI_AGGREGATOR_CHECK};
use crate::review_watch::forge::Repository;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

const MAX_RUNS: usize = 4;

pub(super) fn observe(
    root: &Path,
    repo: &Repository,
    rollup: &Value,
    pr: &Pr,
    base: &str,
) -> Result<Ci, String> {
    let slug = repo.github_slug().ok_or("GitHub repository is unknown")?;
    let (_, project) = slug.split_once('/').ok_or("GitHub project is unknown")?;
    let api = |path: &str| landing_ci_api(root, repo, &format!("repos/{project}/{path}"));
    let (ci, checks) = summarize_github_rollup(rollup, &pr.head, |run| {
        Ok(serde_json::from_slice(&api(&format!(
            "actions/runs/{run}"
        ))?)?)
    });
    verify(ci, &checks, &slug, base, |job| {
        let bytes = api(&format!("actions/jobs/{job}/logs"))?;
        let log = String::from_utf8_lossy(&bytes);
        let merge =
            checkout_sha(&log, pr.id).context("checkout log has no merge commit for this PR")?;
        let commit: Value = serde_json::from_slice(&api(&format!("git/commits/{merge}"))?)?;
        merge_base_parent(&commit, merge, &pr.head).map(str::to_owned)
    })
    .map_err(|error| format!("{error:#}"))
}

/// CI at the exact head from the rollup alone: what the checks concluded,
/// whatever base their runs tested.
// trace:STORY-2906 | ai:claude
pub(super) fn head_ci(
    root: &Path,
    repo: &Repository,
    rollup: &Value,
    pr: &Pr,
) -> Result<Ci, String> {
    let slug = repo.github_slug().ok_or("GitHub repository is unknown")?;
    let (_, project) = slug.split_once('/').ok_or("GitHub project is unknown")?;
    let (ci, _) = summarize_github_rollup(rollup, &pr.head, |run| {
        Ok(serde_json::from_slice(&landing_ci_api(
            root,
            repo,
            &format!("repos/{project}/actions/runs/{run}"),
        )?)?)
    });
    Ok(ci)
}

fn check_link(check: &Value, slug: &str) -> Option<(u64, u64)> {
    let (repo, suffix) = check["detailsUrl"].as_str()?.split_once("/actions/runs/")?;
    if !repo.strip_prefix("https://")?.eq_ignore_ascii_case(slug) {
        return None;
    }
    let (run, job) = suffix.split_once("/job/")?;
    let id = |s: &str| {
        if s.is_empty() || !s.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        s.parse::<u64>().ok().filter(|id| *id != 0)
    };
    Some((id(run)?, id(job)?))
}

fn checkout_job(check: &Value) -> bool {
    // These jobs begin with checkout. The aggregate itself has no checkout.
    matches!(
        check["name"].as_str(),
        Some("CI (gates)" | "CI (fast gates)" | "CI (fast compile)")
    ) && !matches!(
        check_text(check, "conclusion").as_str(),
        "" | "SKIPPED" | "CANCELLED"
    )
}

fn verify(
    ci: Ci,
    checks: &[&Value],
    slug: &str,
    base: &str,
    mut tested_base: impl FnMut(u64) -> Result<String>,
) -> Result<Ci> {
    let mut runs = BTreeSet::new();
    let newest = newest_checks(checks.iter().copied());
    let has_aggregate = newest.iter().any(|check| {
        check_text(check, "workflowName") == "CI" && check_name(check) == CI_AGGREGATOR_CHECK
    });
    for check in newest {
        let conclusion = check_text(check, "conclusion");
        if check_text(check, "workflowName") != "CI"
            || matches!(
                conclusion.as_str(),
                "" | "SKIPPED" | "CANCELLED" | "NEUTRAL"
            )
        {
            continue;
        }
        // The aggregate represents the tested result. Also check failures
        // from other runs that still contribute to the rollup's red result.
        if check_name(check) == CI_AGGREGATOR_CHECK
            || conclusion != "SUCCESS"
            || (!has_aggregate && checkout_job(check))
        {
            let (run, _) =
                check_link(check, slug).context("CI check has no repository-bound run")?;
            runs.insert(run);
        }
    }
    ensure!(runs.len() <= MAX_RUNS, "too many active CI runs to verify");
    let mut missing = None;
    for run in runs {
        let result = checks
            .iter()
            .filter(|check| check_text(check, "workflowName") == "CI" && checkout_job(check))
            .filter_map(|check| check_link(check, slug))
            .filter_map(|(owner, job)| (owner == run).then_some(job))
            .max()
            .context("CI run has no completed checkout job")
            .and_then(&mut tested_base);
        match result {
            Ok(parent) if !parent.eq_ignore_ascii_case(base) => {
                return Ok(Ci::StaleBase(format!(
                    "CI run {run} tested base {}, current base is {}",
                    short(&parent),
                    short(base)
                )));
            }
            Ok(_) => {}
            Err(error) => {
                missing = Some(error.context(format!("CI run {run}'s tested base is unknown")))
            }
        }
    }
    match missing {
        Some(error) => Err(error),
        None => Ok(ci),
    }
}

fn is_sha(sha: &str) -> bool {
    sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn checkout_sha(log: &str, pr: u64) -> Option<&str> {
    // Only consume the first checkout step. Later job output can contain
    // arbitrary text, including strings resembling checkout diagnostics.
    let mut lines = log.lines().filter_map(|line| {
        let (timestamp, body) = line.split_once(' ')?;
        (timestamp.ends_with('Z') && timestamp.contains('T')).then_some(body)
    });
    let first_step = lines.find(|line| line.starts_with("##[group]Run "))?;
    if !first_step.starts_with("##[group]Run actions/checkout@") {
        return None;
    }
    let mut checkout = lines.take_while(|line| !line.starts_with("##[group]Run "));
    let reference = format!("refs/remotes/pull/{pr}/merge");
    let command = checkout.find(|line| line.starts_with("[command]/usr/bin/git checkout "))?;
    if command.split_ascii_whitespace().last() != Some(reference.as_str()) {
        return None;
    }
    checkout.find(|line| *line == "[command]/usr/bin/git log -1 --format=%H")?;
    checkout.next().filter(|sha| is_sha(sha))
}

fn merge_base_parent<'a>(commit: &'a Value, merge: &str, head: &str) -> Result<&'a str> {
    ensure!(
        commit["sha"] == merge,
        "forge returned a different checkout commit"
    );
    let parents = commit["parents"]
        .as_array()
        .context("checkout parents missing")?;
    ensure!(
        parents.len() == 2,
        "CI did not check out a two-parent merge"
    );
    ensure!(
        parents[1]["sha"] == head,
        "CI merge was for a different PR head"
    );
    parents[0]["sha"]
        .as_str()
        .filter(|sha| is_sha(sha))
        .context("invalid first parent")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SLUG: &str = "github.com/o/r";
    const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OLD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const BASE: &str = "cccccccccccccccccccccccccccccccccccccccc";
    const MERGE: &str = "dddddddddddddddddddddddddddddddddddddddd";

    fn check(name: &str, conclusion: &str, run: u64, job: u64, start: &str) -> Value {
        json!({
            "workflowName": "CI",
            "name": name,
            "conclusion": conclusion,
            "startedAt": start,
            "detailsUrl": format!("https://{SLUG}/actions/runs/{run}/job/{job}"),
        })
    }

    #[test]
    fn bug_2690_checkout_log_binds_the_two_parent_commit_to_the_pr_head() {
        let log = format!(
            "2026-11-18T18:33:52.0264683Z ##[group]Run actions/checkout@v5\r\n\
             2026-11-18T18:33:55.3848028Z [command]/usr/bin/git checkout --progress --force refs/remotes/pull/4259/merge\r\n\
             2026-11-18T18:33:55.7678426Z [command]/usr/bin/git log -1 --format=%H\r\n\
             2026-11-18T18:33:55.7704505Z {MERGE}\r\n"
        );
        assert_eq!(checkout_sha(&log, 4259), Some(MERGE));
        assert_eq!(checkout_sha(&log, 4260), None);
        assert_eq!(
            checkout_sha(
                &log.replace(
                    "[command]/usr/bin/git log",
                    "##[group]Run printf\n2026-11-18T18:33:55.7678426Z [command]/usr/bin/git log"
                ),
                4259
            ),
            None
        );
        assert_eq!(
            checkout_sha(&log.replace("actions/checkout@v5", "printf"), 4259),
            None
        );
        assert_eq!(checkout_sha(&log.replace(MERGE, "branch~1"), 4259), None);
        let mut commit = json!({"sha": MERGE, "parents": [{"sha": OLD}, {"sha": HEAD}]});
        assert_eq!(merge_base_parent(&commit, MERGE, HEAD).unwrap(), OLD);
        assert!(merge_base_parent(&commit, BASE, HEAD).is_err());
        assert!(merge_base_parent(&commit, MERGE, BASE).is_err());
        commit["parents"]
            .as_array_mut()
            .unwrap()
            .push(json!({"sha": BASE}));
        assert!(merge_base_parent(&commit, MERGE, HEAD).is_err());
    }

    #[test]
    fn bug_2690_green_and_red_runs_on_the_reverted_base_are_stale() {
        for (ci, conclusion) in [(Ci::Green, "SUCCESS"), (Ci::Red("build".into()), "FAILURE")] {
            let aggregate = check(
                CI_AGGREGATOR_CHECK,
                conclusion,
                20,
                201,
                "2026-11-18T19:00:00Z",
            );
            let checkout = check("CI (gates)", "SUCCESS", 20, 202, "2026-11-18T18:00:00Z");
            let earlier = check(
                "CI (fast gates)",
                "SUCCESS",
                19,
                203,
                "2026-11-18T17:00:00Z",
            );
            let checks = [&earlier, &checkout, &aggregate];
            let read = |job| {
                assert_eq!(job, 202);
                Ok(OLD.into())
            };
            let observed = verify(ci.clone(), &checks, SLUG, BASE, read).unwrap();
            assert!(matches!(observed, Ci::StaleBase(detail) if detail.contains("CI run 20")));
            assert_eq!(verify(ci.clone(), &checks, SLUG, OLD, read).unwrap(), ci);
            assert!(verify(ci, &checks, SLUG, BASE, |_| anyhow::bail!("logs expired")).is_err());
        }
    }

    #[test]
    fn bug_2690_a_failure_contributing_from_a_second_run_checks_its_own_base() {
        let aggregate = check(
            CI_AGGREGATOR_CHECK,
            "SUCCESS",
            20,
            201,
            "2026-11-18T19:00:00Z",
        );
        let checkout = check("CI (gates)", "SUCCESS", 20, 202, "2026-11-18T18:00:00Z");
        let failed = check(
            "CI (fast gates)",
            "FAILURE",
            19,
            193,
            "2026-11-18T17:00:00Z",
        );
        let checks = [&aggregate, &checkout, &failed];
        let read = |job| Ok(if job == 193 { OLD } else { BASE }.into());
        let observed = verify(
            Ci::Red("fast gates".into()),
            &checks,
            "github.com/O/R",
            BASE,
            read,
        )
        .unwrap();
        assert!(matches!(observed, Ci::StaleBase(detail) if detail.contains("CI run 19")));
    }

    #[test]
    fn bug_2690_superseded_runs_and_unrelated_checkout_jobs_supply_no_proof() {
        let stale = check(
            CI_AGGREGATOR_CHECK,
            "FAILURE",
            10,
            101,
            "2026-11-18T17:00:00Z",
        );
        let fresh = check(
            CI_AGGREGATOR_CHECK,
            "SUCCESS",
            11,
            201,
            "2026-11-18T19:00:00Z",
        );
        let checkout = check("CI (gates)", "SUCCESS", 11, 202, "2026-11-18T18:00:00Z");
        let other_run = check("CI (gates)", "SUCCESS", 10, 999, "2026-11-18T16:00:00Z");
        let checks = [&checkout, &stale, &fresh, &other_run];
        assert_eq!(
            verify(Ci::Green, &checks, SLUG, BASE, |job| {
                assert_eq!(job, 202);
                Ok(BASE.into())
            })
            .unwrap(),
            Ci::Green
        );
        assert!(verify(
            Ci::Green,
            &[&fresh, &other_run],
            SLUG,
            BASE,
            |_| unreachable!()
        )
        .is_err());
        assert!(verify(
            Ci::Green,
            &[&fresh],
            "github.com/o/other",
            BASE,
            |_| unreachable!()
        )
        .is_err());
        assert_eq!(
            verify(Ci::None, &[], SLUG, BASE, |_| unreachable!()).unwrap(),
            Ci::None
        );
    }

    // A failed aggregate on an older or unreadable base: the reconciler's
    // answer is stale or unknown, the failed check at the head still stands.
    // trace:STORY-2906 | ai:claude
    #[test]
    fn story_2906_a_red_head_on_a_stale_or_unknown_base_is_still_stuck_merge_red() {
        use super::super::stalls::{standing, Standing};
        use super::super::{derive, Approval, Evidence, Hold, Rounds};
        struct Approved {
            ci: std::result::Result<Ci, String>,
            head: Ci,
        }
        impl Evidence for Approved {
            fn conflict(&self, _: &Pr) -> std::result::Result<bool, String> {
                Ok(false)
            }
            fn reviewing(&self, _: &Pr) -> std::result::Result<Option<String>, String> {
                Ok(None)
            }
            fn ci(&self, _: &Pr) -> std::result::Result<Ci, String> {
                self.ci.clone()
            }
            fn head_red(&self, _: &Pr) -> std::result::Result<Option<String>, String> {
                Ok(match &self.head {
                    Ci::Red(names) => Some(names.clone()),
                    _ => None,
                })
            }
            fn approval(&self, _: &Pr) -> std::result::Result<Approval, String> {
                Ok(Approval::Approved("approved at aaaaaaaa".into()))
            }
            fn hold(&self, _: &Pr) -> std::result::Result<Option<Hold>, String> {
                Ok(None)
            }
            fn rounds(&self, _: &Pr) -> std::result::Result<Rounds, String> {
                Ok(Rounds::default())
            }
            fn lane(&self, _: &Pr) -> std::result::Result<Option<String>, String> {
                Ok(None)
            }
        }
        let rollup = json!([
            check(
                CI_AGGREGATOR_CHECK,
                "FAILURE",
                20,
                201,
                "2026-11-18T19:00:00Z"
            ),
            check("CI (gates)", "FAILURE", 20, 202, "2026-11-18T18:00:00Z"),
        ]);
        let (head, checks) =
            summarize_github_rollup(&rollup, HEAD, |_| anyhow::bail!("no run metadata"));
        let Ci::Red(names) = head.clone() else {
            panic!("the head's checks failed: {head:?}");
        };
        let pr = Pr {
            id: 4344,
            head: HEAD.into(),
            base: "main".into(),
            branch: Some("feature".into()),
            title: "change (TASK-1)".into(),
            updated_at: None,
            hold_label: None,
            ci: None,
        };
        let stale = verify(head.clone(), &checks, SLUG, BASE, |_| Ok(OLD.into()));
        assert!(matches!(stale, Ok(Ci::StaleBase(_))));
        let unknown = verify(head.clone(), &checks, SLUG, BASE, |_| {
            anyhow::bail!("logs expired")
        });
        assert!(unknown.is_err());
        for ci in [stale, unknown] {
            let evidence = Approved {
                ci: ci.map_err(|error| format!("{error:#}")),
                head: head.clone(),
            };
            let row = derive(&pr, &evidence);
            match standing(&pr, &row, &evidence) {
                Standing::Approved {
                    red: Some(red),
                    lane: None,
                    ..
                } => {
                    assert!(red.starts_with(&names), "{red}");
                    assert!(red.contains("base not fresh: "), "{red}");
                }
                other => panic!("expected an approved red head, got {other:?}"),
            }
        }
    }
}
