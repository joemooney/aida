//! Like-for-like consensus reports. Draft filing is explicit and deduplicates under lock.
// trace:STORY-1679 | ai:codex
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

struct Consensus<'a> {
    trials: Vec<&'a score::TrialRecord>,
    failures: BTreeMap<String, usize>,
    models: BTreeSet<&'a str>,
    vendors: BTreeSet<&'a str>,
}

fn discriminating(record: &score::TrialRecord) -> Result<BTreeMap<String, bool>> {
    anyhow::ensure!(
        record.format_version == 1 && record.kind == "trial" && record.status == "scored",
        "consensus requires ordinary scored R1 trials: {}",
        record.trial_id
    );
    let mut checks = BTreeMap::new();
    for check in &record.checks {
        anyhow::ensure!(
            !check.name.is_empty()
                && check.name.trim() == check.name
                && !check.name.chars().any(char::is_control)
                && checks.insert(check.name.clone(), check.passed).is_none(),
            "invalid or duplicate check identity in trial {}",
            record.trial_id
        );
    }
    let mut excluded = HashSet::new();
    for name in &record.excluded_checks {
        anyhow::ensure!(
            checks.contains_key(name) && excluded.insert(name),
            "invalid excluded check set in trial {}",
            record.trial_id
        );
    }
    checks.retain(|name, _| !excluded.contains(name));
    let aggregate = record
        .discriminating
        .as_ref()
        .context("consensus requires a recorded discriminating score")?;
    let passed = checks.values().filter(|p| **p).count();
    anyhow::ensure!(
        !checks.is_empty()
            && aggregate.total == checks.len()
            && aggregate.passed == passed
            && aggregate.pass_rate == passed as f64 / checks.len() as f64,
        "invalid discriminating score in trial {}",
        record.trial_id
    );
    Ok(checks)
}

fn analyze<'a>(records: &'a [score::TrialRecord], ids: &[Uuid]) -> Result<Consensus<'a>> {
    anyhow::ensure!(ids.len() >= 3, "consensus needs at least three trial IDs");
    let mut unique = HashSet::new();
    anyhow::ensure!(
        ids.iter().all(|id| unique.insert(id)),
        "consensus trial IDs must be distinct"
    );
    let mut by_id = BTreeMap::new();
    for record in records {
        anyhow::ensure!(
            by_id.insert(record.trial_id, record).is_none(),
            "duplicate committed trial identity"
        );
    }
    let trials = ids
        .iter()
        .map(|id| {
            by_id
                .get(id)
                .copied()
                .with_context(|| format!("committed trial not found: {id}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let first = trials[0];
    let expected = discriminating(first)?;
    let mut failures: BTreeMap<_, usize> = expected.keys().map(|k| (k.clone(), 0)).collect();
    for trial in &trials {
        anyhow::ensure!(
            trial.oracle_sha == first.oracle_sha
                && trial.oracle_bundle_sha256 == first.oracle_bundle_sha256
                && trial.content_sha256 == first.content_sha256,
            "consensus refuses mixed oracle/bundle/corpus provenance: {}",
            trial.trial_id
        );
        let checks = discriminating(trial)?;
        anyhow::ensure!(
            checks.keys().eq(expected.keys()),
            "consensus refuses differing discriminating check sets: {}",
            trial.trial_id
        );
        for (check, pass) in checks {
            if !pass {
                *failures.get_mut(&check).expect("validated check set") += 1;
            }
        }
    }
    let models = trials.iter().map(|r| r.model.as_str()).collect();
    let vendors = trials.iter().map(|r| r.vendor.as_str()).collect();
    Ok(Consensus {
        trials,
        failures,
        models,
        vendors,
    })
}

impl Consensus<'_> {
    fn gaps(&self) -> Vec<&str> {
        self.failures
            .iter()
            .filter(|(_, count)| **count == self.trials.len())
            .map(|(name, _)| name.as_str())
            .collect()
    }
    fn render(&self) -> String {
        let n = self.trials.len();
        let mut out = format!(
            "Consensus: N={n}, {} distinct models, {} distinct vendors",
            self.models.len(),
            self.vendors.len()
        );
        if self.vendors.len() == 1 {
            out.push_str("\nWarning: all regenerations came from one vendor; consensus is weaker");
        }
        let _ = write!(out, "\nStore gaps (failed by every regeneration):");
        let gaps = self.gaps();
        if gaps.is_empty() {
            out.push_str("\n  none");
        }
        for gap in gaps {
            let _ = write!(
                out,
                "\n  {}: {n}/{n} failures; contract mapping missing",
                gap.escape_debug()
            );
        }
        out.push_str("\nCapability-dependent (some regenerations fail; not store gaps):");
        let mut partial = 0;
        for (name, count) in &self.failures {
            if *count > 0 && *count < n {
                let _ = write!(out, "\n  {}: {count}/{n} failures", name.escape_debug());
                partial += 1;
            }
        }
        if partial == 0 {
            out.push_str("\n  none");
        }
        let _ = write!(
            out,
            "\nCompared trials: {}",
            self.trials
                .iter()
                .map(|r| r.trial_id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        out
    }
}

fn gap_tag(check: &str) -> String {
    format!(
        "regen-gap:{}",
        &format!("{:x}", Sha256::digest(check.as_bytes()))[..16]
    )
}

// The normal Storage atomic path uses GitBackend's store lock and delta writes.
// Do not load/save a snapshot or deduplicate outside the write seam.
fn file_gaps(store: &Path, consensus: &Consensus<'_>) -> Result<Vec<String>> {
    let storage = aida_core::Storage::new(store).with_dispenser(crate::load_dispenser(store)?);
    let mut filed = Vec::new();
    storage.update_atomically(|graph| {
        for check in consensus.gaps() {
            let tag = gap_tag(check);
            if graph.requirements.iter().any(|r| r.status == RequirementStatus::Draft && !r.archived && r.tags.contains(&tag)) { continue; }
            let description = format!("Amend the store after a consensus oracle failure.\n\nExact oracle check (JSON): {}\n\nFailed in every one of {} committed regenerations ({} models, {} vendors). Contract mapping missing; investigate and link the responsible contract clause before amending it. Consensus is evidence for investigation, not proof of cause.\n\nTrial IDs: {}\n\n## Acceptance\n\n- Investigate the named check against the intended contract.\n- If the store omits the required behavior, amend the owning spec with evidence.\n- Otherwise record the oracle defect or capability explanation and close this draft.\n",serde_json::to_string(check).expect("string serialization"),consensus.trials.len(),consensus.models.len(),consensus.vendors.len(),consensus.trials.iter().map(|r|r.trial_id.to_string()).collect::<Vec<_>>().join(", "));
            let mut requirement = Requirement::new(format!("Amend store for consensus failure: {check}"),description);
            requirement.req_type = RequirementType::Task;
            requirement.status = RequirementStatus::Draft;
            requirement.owner = crate::get_default_author();
            requirement.tags.insert(tag);
            requirement.tags.insert("regenerability".into());
            let prefix = graph.get_type_prefix(&RequirementType::Task);
            let id = requirement.id;
            graph.add_requirement_with_id(requirement,None,prefix.as_deref());
            filed.push(graph.get_requirement_by_id(&id).and_then(|r|r.spec_id.clone()).unwrap_or_else(||id.to_string()));
        }
    })?;
    if !filed.is_empty() {
        crate::maybe_auto_push_store(store, crate::StoreAutoPushMode::PerWrite, "per-write");
    }
    Ok(filed)
}

pub(super) fn handle(store: &Path, ids: &[Uuid], file: bool) -> Result<()> {
    // Committed evidence only; validate the entire selection before any writes.
    let records = score::read_committed_trials(store)?;
    let consensus = analyze(&records, ids)?;
    println!("{}", consensus.render());
    if file {
        let filed = file_gaps(store, &consensus)?;
        println!("Draft amendment tasks filed: {}", filed.len());
        for id in filed {
            println!("  {id}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn trial(id: u128, gap: bool, partial: bool) -> score::TrialRecord {
        let pass = (!gap as usize) + (!partial as usize) + 1;
        serde_json::from_value(json!({
            "format_version":1, "trial_id":Uuid::from_u128(id),"store_sha":"s","content_sha256":"corpus",
            "oracle_sha":"oracle","oracle_bundle_sha256":"bundle","model":format!("model-{id}"),
            "vendor":if id == 3 { "vendor B" } else { "vendor A" },"harness":"h",
            "started_at":chrono::Utc::now(),"completed_at":chrono::Utc::now(),"status":"scored","clean":true,
            "checks":[{"name":"gap","passed":!gap},{"name":"partial","passed":!partial},
                      {"name":"pass","passed":true},{"name":"excluded","passed":false}],
            "excluded_checks":["excluded"],
            "discriminating":{"passed":pass,"total":3,"pass_rate":pass as f64/3.0},
            "guard_hits":[],"unconfined":false,"flags":[]
        })).unwrap()
    }
    fn rows() -> Vec<score::TrialRecord> {
        vec![
            trial(1, true, true),
            trial(2, true, false),
            trial(3, true, false),
        ]
    }
    fn ids() -> Vec<Uuid> {
        (1..=3).map(Uuid::from_u128).collect()
    }
    fn store() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "-b", "aida-store"]).unwrap();
        git(root.path(), &["config", "user.name", "Test"]).unwrap();
        git(root.path(), &["config", "user.email", "test@example.com"]).unwrap();
        git(root.path(), &["commit", "--allow-empty", "-m", "fixture"]).unwrap();
        root
    }
    fn commit_rows(store: &Path, rows: &[score::TrialRecord]) {
        std::fs::create_dir_all(store.join("regen/trials")).unwrap();
        for row in rows {
            std::fs::write(
                store.join(format!("regen/trials/{}.json", row.trial_id)),
                serde_json::to_vec(row).unwrap(),
            )
            .unwrap();
        }
        git(store, &["add", "regen/trials"]).unwrap();
        git(store, &["commit", "-m", "trial fixtures"]).unwrap();
    }

    #[test]
    fn consensus_separates_all_fail_from_partial_and_reports_diversity() {
        let rows = rows();
        let ids = ids();
        let report = analyze(&rows, &ids).unwrap();
        assert_eq!(report.gaps(), ["gap"]);
        let out = report.render();
        assert!(out.contains("N=3, 3 distinct models, 2 distinct vendors"));
        assert!(out.contains("gap: 3/3 failures; contract mapping missing"));
        assert!(out.contains("partial: 1/3 failures"));
        assert!(!out.contains("excluded: ") && !out.contains("pass: "));
        assert!(!out.contains("consensus is weaker"));
        let mut rows = rows;
        for row in &mut rows {
            row.vendor = "one vendor".into();
            row.model = "one model".into();
        }
        let out = analyze(&rows, &ids).unwrap().render();
        assert!(out.contains("1 distinct models, 1 distinct vendors"));
        assert!(out.contains("consensus is weaker"));
    }

    #[test]
    fn consensus_refuses_mixed_provenance_sets_invalid_scores_or_nonordinary_attempts() {
        for variant in 0..9 {
            let mut rows = rows();
            match variant {
                0 => rows[1].oracle_sha = "other".into(),
                1 => rows[1].oracle_bundle_sha256 = "other".into(),
                2 => rows[1].content_sha256 = "other".into(),
                3 => rows[1].checks[0].name = "different".into(),
                4 => rows[1].kind = "control-reference".into(),
                5 => rows[1].kind = "trial-adapted".into(),
                6 => rows[1].status = "install_failed".into(),
                7 => rows[1].discriminating.as_mut().unwrap().pass_rate = 0.99,
                _ => rows[1].excluded_checks.push("nonexistent".into()),
            }
            assert!(analyze(&rows, &ids()).is_err(), "variant {variant}");
        }
        assert!(analyze(&rows(), &ids()[..2]).is_err());
        assert!(analyze(&rows(), &[Uuid::from_u128(1); 3]).is_err());
        assert!(analyze(
            &rows(),
            &[Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(4)]
        )
        .is_err());
    }

    #[test]
    fn explicit_file_is_idempotent_and_report_default_does_not_write() {
        let store = store();
        let rows = rows();
        commit_rows(store.path(), &rows);
        let sha = head(store.path()).unwrap();
        handle(store.path(), &ids(), false).unwrap();
        assert_eq!(head(store.path()).unwrap(), sha);
        assert!(!store.path().join("objects").exists());
        handle(store.path(), &ids(), true).unwrap();
        let storage = aida_core::Storage::new(store.path());
        let graph = storage.load().unwrap();
        assert_eq!(graph.requirements.len(), 1);
        let gap = &graph.requirements[0];
        assert_eq!(gap.status, RequirementStatus::Draft);
        assert_eq!(gap.req_type, RequirementType::Task);
        assert!(gap.tags.contains(&gap_tag("gap")));
        assert!(gap
            .description
            .contains("Exact oracle check (JSON): \"gap\""));
        assert!(gap.description.contains("Contract mapping missing"));
        assert!(gap.relationships.is_empty());
        let first_id = gap.id;
        handle(store.path(), &ids(), true).unwrap();
        let graph = storage.load().unwrap();
        assert_eq!(graph.requirements.len(), 1);
        assert_eq!(graph.requirements[0].id, first_id);
        // Once the old draft is closed, the same check can file a new investigation.
        storage
            .update_atomically(|graph| graph.requirements[0].status = RequirementStatus::Rejected)
            .unwrap();
        handle(store.path(), &ids(), true).unwrap();
        assert_eq!(storage.load().unwrap().requirements.len(), 2);
        let mut mixed = rows;
        mixed[2].oracle_sha = "other".into();
        commit_rows(store.path(), &mixed);
        let before = head(store.path()).unwrap();
        assert!(handle(store.path(), &ids(), true).is_err());
        assert_eq!(head(store.path()).unwrap(), before);
    }

    #[test]
    fn consensus_reads_committed_trials_and_cli_excludes_execution_flags() {
        use clap::Parser;
        let store = store();
        commit_rows(store.path(), &rows());
        let path = store.path().join(format!("regen/trials/{}.json", ids()[0]));
        std::fs::write(&path, "invalid dirty JSON").unwrap();
        handle(store.path(), &ids(), false).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid dirty JSON");
        let texts: Vec<_> = ids().iter().map(Uuid::to_string).collect();
        let mut args = vec!["aida", "regen", "score", "--consensus"];
        args.extend(texts.iter().map(String::as_str));
        assert!(crate::cli::Cli::try_parse_from(&args).is_ok());
        let mut file = args.clone();
        file.push("--file");
        assert!(crate::cli::Cli::try_parse_from(file).is_ok());
        for extra in [
            vec!["system"],
            vec!["--control", "stub"],
            vec!["--corpus", "corpus"],
            vec!["--model", "model"],
            vec!["--vendor", "vendor"],
            vec!["--harness", "h"],
            vec!["--unconfined"],
        ] {
            let mut bad = args.clone();
            bad.extend(extra);
            assert!(crate::cli::Cli::try_parse_from(bad).is_err());
        }
        assert!(
            crate::cli::Cli::try_parse_from(["aida", "regen", "score", "system", "--file"])
                .is_err()
        );
        assert!(crate::cli::Cli::try_parse_from(["aida", "regen", "score", "system"]).is_err());
    }
}
