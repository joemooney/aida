//! STORY-2894 S1: the generated files the merge train may regenerate at its
//! tip, and the checks on the one regeneration commit it writes. The
//! regeneration commands come from the host-embedded `REGISTRY` in
//! `scripts/repair-generated.py` (a ratchet test keeps both tables equal);
//! train eligibility comes only from this host-compiled table, on the
//! protected floor. Approval coverage is unchanged in S1.
// trace:TASK-3963 | ai:claude

use crate::gate::mandate_gates::{touches_gates, ProtectedPaths};

/// How an output relates to its inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrainClass {
    /// Every byte is generator output.
    Derived,
    /// The generator owns the rows; each row carries reviewed policy.
    KeyedLedger,
}

pub(crate) struct TrainEntry {
    pub name: &'static str,
    pub outputs: &'static [&'static str],
    pub inputs: &'static [&'static str],
    pub argv: &'static [&'static str],
    /// The generator's own files. The train runs the base's copy; a car that
    /// changes one disables the entry for that train.
    pub generators: &'static [&'static str],
    pub class: TrainClass,
    /// The train runs it. `cli-help` needs a cargo build (S3 decides).
    pub train_run: bool,
}

pub(crate) const TRAIN_GENERATED: &[TrainEntry] = &[
    TrainEntry {
        name: "showcase-search",
        outputs: &["docs/showcase/search-data.js"],
        inputs: &["docs/", "aida-core/templates/.aida/discipline/"],
        argv: &["python3", "docs/showcase/build.py"],
        generators: &["docs/showcase/build.py"],
        class: TrainClass::Derived,
        train_run: true,
    },
    TrainEntry {
        name: "store-writers",
        outputs: &["scripts/store-writers.toml"],
        inputs: &["**/*.rs", "scripts/", "aida-core/templates/", "hooks/", ".githooks/"],
        argv: &["python3", "scripts/check-store-writers.py", "--regenerate"],
        generators: &["scripts/check-store-writers.py"],
        class: TrainClass::KeyedLedger,
        train_run: true,
    },
    TrainEntry {
        name: "cli-help",
        outputs: &["aida-cli-lib/src/tests/golden/cli_help_snapshot.txt"],
        inputs: &["aida-cli-lib/src/", "Cargo.toml", "Cargo.lock"],
        argv: &[
            "env", "AIDA_UPDATE_SNAPSHOTS=1", "cargo", "test", "--locked", "--profile", "agent",
            "-p", "aida-cli-lib", "--lib", "--", "--exact",
            "command_registry::story_1728_command_registry_tests::help_surface_matches_golden_snapshot",
        ],
        generators: &["aida-cli-lib/src/", "Cargo.toml", "Cargo.lock"],
        class: TrainClass::Derived,
        train_run: false,
    },
];

pub(crate) const REGEN_SUBJECT: &str = "[AI:aida-train] chore(train): regenerate generated files";
pub(crate) const REGEN_BLOCK: &str = "aida-train-regenerated:";

/// One output in the driver's `train` report (sha256 before and after).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RegenOutput {
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RegenEntry {
    pub name: String,
    pub argv: Vec<String>,
    pub status: i32,
    pub outputs: Vec<RegenOutput>,
}

/// The train's regeneration commit on top of its last car (journaled).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RegenCommit {
    pub commit: String,
    pub report: Vec<RegenEntry>,
}

/// The entries the train may run. An entry whose output the effective
/// protected set covers is disabled with a warning, never widened.
pub(crate) fn runnable(protected: &ProtectedPaths) -> (Vec<&'static TrainEntry>, Vec<String>) {
    let mut warnings = Vec::new();
    let entries = TRAIN_GENERATED
        .iter()
        .filter(|e| e.train_run)
        .filter(|e| {
            let outputs: Vec<String> = e.outputs.iter().map(|p| p.to_string()).collect();
            let hit = touches_gates(&outputs, protected);
            if let Some(hit) = &hit {
                warnings.push(format!(
                    "train regeneration of {} disabled: {} is protected by `{}`",
                    e.name, hit.path, hit.entry
                ));
            }
            hit.is_none()
        })
        .collect();
    (entries, warnings)
}

fn input_matches(pattern: &str, path: &str) -> bool {
    match pattern.strip_prefix("**/*") {
        Some(suffix) => path.ends_with(suffix),
        None if pattern.ends_with('/') => path.starts_with(pattern),
        None => path == pattern,
    }
}

/// The entries whose generator no car changes. Generators always run from
/// the train base; a car that edits one keeps that entry's strict check.
pub(crate) fn base_trusted(
    entries: &[&'static TrainEntry],
    car_paths: &[Vec<String>],
) -> (Vec<&'static TrainEntry>, Vec<String>) {
    let mut warnings = Vec::new();
    let trusted = entries
        .iter()
        .copied()
        .filter(|e| {
            let changed = car_paths
                .iter()
                .flatten()
                .find(|p| e.generators.iter().any(|g| input_matches(g, p)));
            if let Some(path) = changed {
                warnings.push(format!(
                    "train regeneration of {} disabled: a car changes its generator {path}",
                    e.name
                ));
            }
            changed.is_none()
        })
        .collect();
    (trusted, warnings)
}

/// A refusal reason as one bounded line, for the journal and PR body.
pub(crate) fn refusal_line(why: &str) -> String {
    let line = why.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(400) {
        Some((at, _)) => format!("{}...", &line[..at]),
        None => line,
    }
}

/// Entries to run: those whose inputs two or more cars touched.
pub(crate) fn select(
    entries: &[&'static TrainEntry],
    car_paths: &[Vec<String>],
) -> Vec<&'static str> {
    entries
        .iter()
        .filter(|e| {
            let touched = |paths: &Vec<String>| {
                paths
                    .iter()
                    .any(|p| e.inputs.iter().any(|i| input_matches(i, p)))
            };
            car_paths.iter().filter(|paths| touched(paths)).count() >= 2
        })
        .map(|e| e.name)
        .collect()
}

/// The report covers exactly `selected`, with the table's argv and outputs.
pub(crate) fn check_report(selected: &[&str], report: &[RegenEntry]) -> Result<(), String> {
    let names: Vec<&str> = report.iter().map(|e| e.name.as_str()).collect();
    if names != selected {
        return Err(format!(
            "regeneration report names {names:?}, expected {selected:?}"
        ));
    }
    for got in report {
        let want = TRAIN_GENERATED
            .iter()
            .find(|e| e.name == got.name && e.train_run);
        let paths: Vec<&str> = got.outputs.iter().map(|o| o.path.as_str()).collect();
        match want {
            Some(w) if got.status == 0 && got.argv == w.argv && paths == w.outputs => {}
            _ => {
                return Err(format!(
                    "regeneration report entry {} is not canonical",
                    got.name
                ))
            }
        }
    }
    Ok(())
}

/// The outputs whose bytes changed, in report order.
pub(crate) fn changed(report: &[RegenEntry]) -> Vec<(&RegenEntry, &RegenOutput)> {
    report
        .iter()
        .flat_map(|e| e.outputs.iter().map(move |o| (e, o)))
        .filter(|(_, o)| o.before != o.after)
        .collect()
}

fn hash(h: &Option<String>) -> &str {
    h.as_deref().unwrap_or("absent")
}

/// The canonical regeneration commit message. No `(ID)`: it completes nothing.
pub(crate) fn regen_message(report: &[RegenEntry]) -> String {
    let mut trailers = Vec::new();
    let mut commands = Vec::new();
    for (e, o) in changed(report) {
        let (b, a) = (hash(&o.before), hash(&o.after));
        trailers.push(format!(
            "Aida-Train-Regenerated: {} {} {b} -> {a}",
            e.name, o.path
        ));
        let command = format!("Aida-Train-Regen-Command: {} {}", e.name, e.argv.join(" "));
        if !commands.contains(&command) {
            commands.push(command);
        }
    }
    trailers.extend(commands);
    format!("{REGEN_SUBJECT}\n\n{}\n", trailers.join("\n"))
}

/// The train PR body block recording each regenerated output.
pub(crate) fn pr_body_block(regen: &RegenCommit) -> String {
    let mut s = format!("{REGEN_BLOCK} {}\n", regen.commit);
    for (e, o) in changed(&regen.report) {
        let (b, a) = (hash(&o.before), hash(&o.after));
        s.push_str(&format!(
            "- {} `{}` {b} -> {a} (`{}`)\n",
            e.name,
            o.path,
            e.argv.join(" ")
        ));
    }
    s
}

/// `verify_structure`'s check of the trailing regeneration commit, given its
/// message and the paths it changes relative to the last car. Runs no
/// generator.
pub(crate) fn verify_regen(
    message: &str,
    touched: &[String],
    regen: &RegenCommit,
) -> Result<(), String> {
    let selected: Vec<&str> = regen.report.iter().map(|e| e.name.as_str()).collect();
    check_report(&selected, &regen.report)?;
    if message.trim_end() != regen_message(&regen.report).trim_end() {
        return Err("train regeneration commit message changed".to_string());
    }
    let mut want: Vec<String> = changed(&regen.report)
        .iter()
        .map(|(_, o)| o.path.clone())
        .collect();
    let mut got = touched.to_vec();
    want.sort();
    got.sort();
    if want.is_empty() || got != want {
        return Err(format!(
            "train regeneration commit touches {got:?}, expected {want:?}"
        ));
    }
    Ok(())
}

/// E2 post-condition on the regenerated ledger: reviewed rows unchanged and
/// new rows `blocked`.
pub(crate) fn ledger_violation(before: &str, after: &str) -> Option<String> {
    let rows = |text: &str| -> Option<std::collections::BTreeMap<String, toml::Value>> {
        let value: toml::Table = text.parse().ok()?;
        let rows = value.get("writer")?.as_array()?;
        rows.iter()
            .map(|r| Some((r.get("id")?.as_str()?.to_string(), r.clone())))
            .collect()
    };
    let (Some(before), Some(after)) = (rows(before), rows(after)) else {
        return Some("store-writers inventory does not parse".to_string());
    };
    after.iter().find_map(|(id, row)| match before.get(id) {
        Some(old) if old != row => Some(format!("regeneration changed reviewed row {id}")),
        None if row.get("disposition").and_then(|d| d.as_str()) != Some("blocked") => {
            Some(format!("regeneration added a non-blocked row {id}"))
        }
        _ => None,
    })
}

#[cfg(test)]
#[path = "../tests/task_3963_train_generated_tests.rs"]
mod tests;
