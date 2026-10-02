//! AC5 of TASK-1555: keep the local CI-gate runner from drifting out of date.
//!
//! `make check-ci-fast` / `make check-ci` run CI's own `run:` bodies, read out
//! of `.github/workflows/ci.yml`, so a gate whose COMMAND changes cannot drift.
//! What can still drift is the gate SET: someone adds a step to the Build job
//! and `scripts/ci-gate-tiers.toml` never learns about it, so the local tier
//! silently stops covering it. That silent omission is the root cause the spec
//! names, so it is guarded here rather than trusted.
//!
//! This lives in `cargo test --workspace`, which CI already runs, on purpose. A
//! shell or python guard would have to be named individually as its own step in
//! `ci.yml` — and a fixture can sit on a branch wired into nothing while looking
//! like coverage.
//!
//! Both files are read AT RUNTIME from `CARGO_MANIFEST_DIR`. `include_str!`
//! would compile a copy of each into the test binary, which makes the only
//! mutation proof that matters — delete the table, delete an entry — impossible
//! to write.
//
// trace:TASK-1555 | ai:claude

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("aida-cli-lib always has a workspace parent")
        .to_path_buf()
}

/// Every named `run:` step of CI's Build job — the gate set the local runner
/// owes coverage for.
fn ci_build_gate_names(root: &Path) -> Vec<String> {
    let path = root.join(".github/workflows/ci.yml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("{} must be readable: {err}", path.display()));
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&text).unwrap_or_else(|err| panic!("ci.yml must parse: {err}"));
    let steps = workflow
        .get("jobs")
        .and_then(|jobs| jobs.get("build"))
        .and_then(|build| build.get("steps"))
        .and_then(|steps| steps.as_sequence())
        .expect("ci.yml must have jobs.build.steps");
    steps
        .iter()
        .filter(|step| step.get("run").is_some())
        .filter_map(|step| step.get("name")?.as_str().map(str::to_string))
        .collect()
}

/// `(tiered, skipped)` from the classification table.
fn classified_gate_names(root: &Path) -> (BTreeSet<String>, BTreeSet<String>) {
    let path = root.join("scripts/ci-gate-tiers.toml");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "{} must exist: {err}\n\
             It is the only record of which CI gates run locally. Without it \
             `make check-ci` covers nothing, so its absence is a failure, not a pass.",
            path.display()
        )
    });
    let table: toml::Value = text
        .parse()
        .unwrap_or_else(|err| panic!("ci-gate-tiers.toml must parse: {err}"));
    let names = |key: &str| -> BTreeSet<String> {
        table
            .get(key)
            .and_then(|v| v.as_table())
            .map(|t| t.keys().cloned().collect())
            .unwrap_or_default()
    };
    (names("tiers"), names("skips"))
}

/// A new CI gate must be classified, and a classified gate must still exist in
/// CI. Both directions matter: the first catches an added gate the local tier
/// never learns about, the second catches a RENAMED one, which is the same
/// silent loss of coverage wearing a different hat.
#[test]
fn task_1555_every_ci_build_gate_is_classified_for_the_local_runner() {
    let root = workspace_root();
    let ci_names = ci_build_gate_names(&root);
    assert!(
        ci_names.len() > 20,
        "expected CI's Build job to carry the gate set this guard is about; \
         found only {} named `run:` steps — the parse is probably wrong, and a \
         guard that reads nothing passes everything",
        ci_names.len()
    );

    let (tiered, skipped) = classified_gate_names(&root);
    let classified: BTreeSet<String> = tiered.union(&skipped).cloned().collect();
    let ci_set: BTreeSet<String> = ci_names.iter().cloned().collect();

    let unclassified: Vec<&String> = ci_names
        .iter()
        .filter(|n| !classified.contains(*n))
        .collect();
    assert!(
        unclassified.is_empty(),
        "{} CI Build gate(s) are not classified in scripts/ci-gate-tiers.toml, so \
         `make check-ci` silently does not run them:\n{}\n\n\
         Add each to [tiers] as \"fast\" (needs no cargo build) or \"full\", or to \
         [skips] with a one-line reason it cannot run locally.",
        unclassified.len(),
        unclassified
            .iter()
            .map(|n| format!("  - {n}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let stale: Vec<&String> = classified.iter().filter(|n| !ci_set.contains(*n)).collect();
    assert!(
        stale.is_empty(),
        "{} classified gate(s) name no step in CI's Build job — a renamed or deleted \
         step leaves a stale entry, and the gate stops running locally with nothing \
         failing:\n{}",
        stale.len(),
        stale
            .iter()
            .map(|n| format!("  - {n}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let both: Vec<&String> = tiered.intersection(&skipped).collect();
    assert!(
        both.is_empty(),
        "gate(s) are both tiered and skipped, so the runner's answer depends on \
         table order: {both:?}"
    );
}

/// The tier values the runner understands are exactly `fast` and `full`. A typo
/// such as `"Fast"` would make `tiers.get(name)` miss every wanted tier, and the
/// gate would be skipped — quietly, which is the failure mode this spec exists
/// to remove.
#[test]
fn task_1555_every_tier_value_is_one_the_runner_runs() {
    let root = workspace_root();
    let path = root.join("scripts/ci-gate-tiers.toml");
    let table: toml::Value = std::fs::read_to_string(&path)
        .expect("ci-gate-tiers.toml must exist")
        .parse()
        .expect("ci-gate-tiers.toml must parse");
    let tiers = table
        .get("tiers")
        .and_then(|v| v.as_table())
        .expect("ci-gate-tiers.toml must have a [tiers] table");
    let bad: Vec<String> = tiers
        .iter()
        .filter(|(_, value)| !matches!(value.as_str(), Some("fast") | Some("full")))
        .map(|(name, value)| format!("  - {name} = {value}"))
        .collect();
    assert!(
        bad.is_empty(),
        "tier must be \"fast\" or \"full\"; `scripts/check-ci-gates.py` runs neither \
         of these and would skip the gate without saying why:\n{}",
        bad.join("\n")
    );
    assert!(
        tiers.values().any(|v| v.as_str() == Some("fast")),
        "no gate is tiered `fast`, so `make check-ci-fast` would run nothing and \
         report success"
    );
}

/// The runner reproduces CI's shell exactly, and says so in `CI_SHELL_CONTRACT`.
/// If CI's `defaults.run.shell` moves and the runner is not updated, every local
/// green becomes a claim about a shell CI no longer uses. The runner refuses at
/// runtime; this makes the drift fail in CI too, next to the gate table it
/// belongs with.
#[test]
fn task_1555_runner_mirrors_cis_declared_shell() {
    let root = workspace_root();
    let workflow: serde_yaml::Value = serde_yaml::from_str(
        &std::fs::read_to_string(root.join(".github/workflows/ci.yml")).expect("ci.yml readable"),
    )
    .expect("ci.yml parses");
    let ci_shell = workflow
        .get("jobs")
        .and_then(|j| j.get("build"))
        .and_then(|b| b.get("defaults"))
        .and_then(|d| d.get("run"))
        .and_then(|r| r.get("shell"))
        .and_then(|s| s.as_str())
        .expect("CI's build job must declare defaults.run.shell");

    let runner = std::fs::read_to_string(root.join("scripts/check-ci-gates.py"))
        .expect("check-ci-gates.py must exist");
    let declared = runner
        .lines()
        .find_map(|line| line.strip_prefix("CI_SHELL_CONTRACT = "))
        .map(|value| value.trim().trim_matches('"'))
        .expect("check-ci-gates.py must declare CI_SHELL_CONTRACT");

    assert_eq!(
        declared, ci_shell,
        "scripts/check-ci-gates.py mirrors CI's shell as `{declared}`, but \
         ci.yml now declares `{ci_shell}`. Local parity is a claim about CI's \
         shell; update the runner, or the claim is false."
    );
}
