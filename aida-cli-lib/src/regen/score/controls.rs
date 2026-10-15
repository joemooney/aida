//! Control batches are bound to the exact oracle, corpus, and check identities.
// trace:TASK-1731 | ai:codex
use super::*;

pub(super) struct RunSetup {
    pub(super) kind: &'static str,
    pub(super) run_id: Option<Uuid>,
    pub(super) repeat: Option<usize>,
    pub(super) repeats: Option<usize>,
}

pub(super) struct ControlGate {
    names: Vec<String>,
    excluded: HashSet<String>,
    ids: Vec<Uuid>,
    guard_hash: String,
}

fn names(record: &TrialRecord) -> Result<Vec<String>> {
    let mut names: Vec<_> = record.checks.iter().map(|c| c.name.clone()).collect();
    anyhow::ensure!(
        names.iter().all(|name| !name.is_empty()
            && name.trim() == name
            && !name.chars().any(char::is_control)),
        "control has invalid check identities"
    );
    names.sort();
    anyhow::ensure!(
        !names.is_empty() && names.windows(2).all(|w| w[0] != w[1]),
        "control has empty or duplicate check identities"
    );
    let parsed = parse_checks(
        &record
            .checks
            .iter()
            .map(|c| {
                format!(
                    "{}{}\n",
                    if c.passed { "ok   - " } else { "FAIL - " },
                    c.name
                )
            })
            .collect::<String>(),
    )?;
    anyhow::ensure!(
        parsed.len() == record.checks.len(),
        "control has invalid check identities"
    );
    Ok(names)
}

fn require_execution(
    record: &TrialRecord,
    label: &str,
    control: bool,
    guard_hash: &str,
) -> Result<()> {
    let nonzero = record.flags.iter().any(|f| f == "oracle_nonzero_exit");
    let flags_valid = record
        .flags
        .iter()
        .all(|f| !control && f == "oracle_nonzero_exit");
    anyhow::ensure!(
        record.format_version == 1 && record.status == "scored"
            && record.clean == record.flags.is_empty() && record.root_fresh
            && !record.unconfined && flags_valid && record.guard_hits.is_empty()
            && record.survivors_detected == 0
            && record.env_executable.as_deref() == Some("/usr/bin/env")
            && record.guard_bundle_sha256.as_deref() == Some(guard_hash)
            && ((record.oracle_exit == Some(0)) == !nonzero)
            && (!control || record.oracle_exit == Some(0))
            && record.install_exit.is_none_or(|e| e == 0) && record.error.is_none(),
        "{label} failed clean-sandbox control (fresh root, confinement, exits, flags and guards must be clean)"
    );
    let passed = record.checks.iter().filter(|c| c.passed).count();
    let total = record.checks.len();
    anyhow::ensure!(
        record.aggregate.as_ref().is_some_and(|a| a.passed == passed
            && a.total == total
            && total > 0
            && a.pass_rate == passed as f64 / total as f64),
        "{label} control aggregate does not match checks"
    );
    Ok(())
}

pub(super) fn read_records(store: &Path) -> Result<Vec<TrialRecord>> {
    // Only committed evidence counts; pin once to avoid mixing concurrent writes.
    let sha = super::super::head(store)?;
    let rows = super::super::git(store, &["ls-tree", "-rz", &sha, "--", "regen/trials/"])?;
    let mut records = Vec::new();
    for row in rows.split(|b| *b == 0).filter(|b| !b.is_empty()) {
        let row = std::str::from_utf8(row)?;
        let (meta, path) = row.split_once('\t').context("invalid trial tree entry")?;
        let fields: Vec<_> = meta.split_whitespace().collect();
        anyhow::ensure!(
            fields.len() == 3 && fields[0] == "100644" && fields[1] == "blob",
            "unsafe control record: {path}"
        );
        let bytes = super::super::git(store, &["cat-file", "blob", fields[2]])?;
        let record: TrialRecord = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid control/trial record: {path}"))?;
        anyhow::ensure!(
            path == format!("regen/trials/{}.json", record.trial_id),
            "control record identity/path mismatch: {path}"
        );
        records.push(record);
    }
    Ok(records)
}

impl ControlGate {
    pub(super) fn load(
        store: &Path,
        bundle: &OracleBundle,
        manifest: &Manifest,
        repeats: usize,
        guard_hash: &str,
    ) -> Result<Self> {
        Self::from_records(
            read_records(store)?,
            &bundle.sha,
            &bundle.hash,
            &manifest.content_sha256,
            repeats,
            guard_hash,
        )
    }

    fn from_records(
        records: Vec<TrialRecord>,
        sha: &str,
        bundle: &str,
        corpus: &str,
        repeats: usize,
        guard_hash: &str,
    ) -> Result<Self> {
        anyhow::ensure!(repeats >= 1, "reference_repeats must be >= 1");
        let records: Vec<_> = records
            .into_iter()
            .filter(|r| {
                r.oracle_sha == sha
                    && r.oracle_bundle_sha256 == bundle
                    && r.content_sha256 == corpus
            })
            .collect();
        let latest = records
            .iter()
            .filter(|r| r.kind == "control-reference")
            .max_by_key(|r| (r.started_at, r.trial_id))
            .context("missing matching reference control (oracle/corpus provenance)")?;
        let run_id = latest
            .control_run_id
            .context("reference control lacks a complete batch identity")?;
        let batch: Vec<_> = records
            .iter()
            .filter(|r| r.kind == "control-reference" && r.control_run_id == Some(run_id))
            .collect();
        anyhow::ensure!(
            batch.len() == repeats && batch.iter().all(|r| r.control_repeats == Some(repeats)),
            "reference control batch is incomplete or repeat count changed: need {repeats} repeats"
        );
        let ordinals: HashSet<_> = batch.iter().filter_map(|r| r.control_repeat).collect();
        anyhow::ensure!(
            ordinals.len() == repeats && (1..=repeats).all(|i| ordinals.contains(&i)),
            "reference control batch has missing/duplicate repeat identities"
        );
        let expected = names(batch[0])?;
        let mut ids = Vec::new();
        for record in batch {
            require_execution(record, "reference", true, guard_hash)?;
            anyhow::ensure!(
                record.checks.iter().all(|c| c.passed),
                "reference control failed: every repeat must pass 100%"
            );
            anyhow::ensure!(
                names(record)? == expected,
                "reference control check-name set changed between repeats"
            );
            ids.push(record.trial_id);
        }
        let stub = records
            .iter()
            .filter(|r| r.kind == "control-stub")
            .max_by_key(|r| (r.started_at, r.trial_id))
            .context("missing matching stub control (oracle/corpus provenance)")?;
        require_execution(stub, "stub", true, guard_hash)?;
        anyhow::ensure!(
            stub.stub_tools.as_deref() == Some("generated-noop") && stub.install_exit.is_none(),
            "stub control must use generated no-op tools without a project installer"
        );
        anyhow::ensure!(
            names(stub)? == expected,
            "stub control check-name set differs from reference"
        );
        let excluded: HashSet<_> = stub
            .checks
            .iter()
            .filter(|c| c.passed)
            .map(|c| c.name.clone())
            .collect();
        anyhow::ensure!(
            excluded.len() < expected.len(),
            "empty discriminating denominator: stub passes every check"
        );
        ids.push(stub.trial_id);
        ids.sort();
        Ok(Self {
            names: expected,
            excluded,
            ids,
            guard_hash: guard_hash.into(),
        })
    }

    pub(super) fn apply(&self, record: &mut TrialRecord) -> Result<()> {
        require_execution(record, "trial", false, &self.guard_hash)?;
        anyhow::ensure!(
            names(record)? == self.names,
            "trial check-name set differs from controls"
        );
        let checks: Vec<_> = record
            .checks
            .iter()
            .filter(|c| !self.excluded.contains(&c.name))
            .collect();
        let total = checks.len();
        anyhow::ensure!(total > 0, "empty discriminating denominator");
        let passed = checks.iter().filter(|c| c.passed).count();
        record.discriminating = Some(Aggregate {
            passed,
            total,
            pass_rate: passed as f64 / total as f64,
        });
        record.excluded_checks = self.excluded.iter().cloned().collect();
        record.excluded_checks.sort();
        record.control_ids = self.ids.clone();
        Ok(())
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;

    fn record(kind: &str, run: Uuid, ordinal: usize, values: &[(&str, bool)]) -> TrialRecord {
        let passed = values.iter().filter(|(_, pass)| *pass).count();
        TrialRecord {
            gaps: None,
            format_version: 1,
            adapter: None,
            trial_id: Uuid::new_v4(),
            kind: kind.into(),
            root_fresh: true,
            control_run_id: Some(run),
            control_repeat: Some(ordinal),
            control_repeats: Some(3),
            stub_tools: (kind == "control-stub").then(|| "generated-noop".into()),
            discriminating: None,
            excluded_checks: vec![],
            control_ids: vec![],
            content_sha256: "corpus".into(),
            store_sha: "store".into(),
            oracle_sha: "oracle".into(),
            oracle_bundle_sha256: "bundle".into(),
            model: "m".into(),
            model_family: None,
            failure_diagnostics: None,
            vendor: "v".into(),
            harness: "h".into(),
            started_at: Utc::now(),
            completed_at: Utc::now(),
            checks: values
                .iter()
                .map(|(n, p)| CheckOutcome {
                    name: (*n).into(),
                    passed: *p,
                })
                .collect(),
            aggregate: Some(Aggregate {
                passed,
                total: values.len(),
                pass_rate: passed as f64 / values.len() as f64,
            }),
            install_exit: None,
            oracle_exit: Some(0),
            status: "scored".into(),
            clean: true,
            guard_hits: vec![],
            unconfined: false,
            survivors_detected: 0,
            env_executable: Some("/usr/bin/env".into()),
            guard_bundle_sha256: Some("guards".into()),
            flags: vec![],
            error: None,
        }
    }
    fn records() -> Vec<TrialRecord> {
        let run = Uuid::new_v4();
        let mut rows: Vec<_> = (1..=3)
            .map(|i| {
                record(
                    "control-reference",
                    run,
                    i,
                    &[("behavior", true), ("hollow", true)],
                )
            })
            .collect();
        rows.push(record(
            "control-stub",
            Uuid::new_v4(),
            1,
            &[("behavior", false), ("hollow", true)],
        ));
        rows
    }
    fn gate(rows: Vec<TrialRecord>) -> Result<ControlGate> {
        ControlGate::from_records(rows, "oracle", "bundle", "corpus", 3, "guards")
    }
    fn refused(rows: Vec<TrialRecord>, expected: &str) {
        let error = match gate(rows) {
            Ok(_) => panic!("accepted invalid controls"),
            Err(e) => e,
        };
        assert!(format!("{error:#}").contains(expected), "{error:#}");
    }
    // trace:TASK-1732 | ai:codex
    #[test]
    fn adapter_applies_same_controls_to_both_runs_and_retains_failure_evidence() {
        let gate = gate(records()).unwrap();
        for failure in 0..6 {
            let mut r1 = record(
                "trial",
                Uuid::new_v4(),
                1,
                &[("behavior", false), ("hollow", true)],
            );
            let mut r2 = record(
                "trial-adapted",
                Uuid::new_v4(),
                1,
                &[("behavior", true), ("hollow", true)],
            );
            match failure {
                1 => {
                    r2.guard_hits.push("tool".into());
                    r2.flags.push("guard_hits".into());
                    r2.clean = false;
                }
                2 => {
                    r2.status = "install_failed".into();
                    r2.install_exit = Some(3);
                    r2.oracle_exit = None;
                    r2.aggregate = None;
                    r2.checks.clear();
                    r2.error = Some("install diagnostic".into());
                    r2.clean = false;
                }
                3 => {
                    r2.checks[0].name = "changed".into();
                }
                4 => {
                    r2.survivors_detected = 1;
                    r2.clean = false;
                }
                5 => {
                    r2.oracle_exit = Some(7);
                    r2.flags.push("oracle_nonzero_exit".into());
                    r2.clean = false;
                }
                _ => {}
            }
            adapter::apply_gate(&mut r1, Some(&gate));
            adapter::apply_gate(&mut r2, Some(&gate));
            let root = tempfile::tempdir().unwrap();
            std::fs::write(root.path().join("GAPS.toml"), "format_version=1").unwrap();
            let capture = adapter::CapturedAdapter::load(root.path())
                .unwrap()
                .unwrap();
            capture.attach(&mut r1, r2);
            let value = serde_json::to_value(&r1).unwrap();
            if failure == 0 || failure == 5 {
                assert_eq!(r1.status, "scored");
                assert_eq!(value["adapter"]["raw_delta"], 0.5);
                assert_eq!(value["adapter"]["discriminating_delta"], 1.0);
                assert_eq!(
                    value["adapter"]["r2"]["excluded_checks"],
                    serde_json::json!(["hollow"])
                );
                assert_eq!(value["adapter"]["r2"]["control_ids"], value["control_ids"]);
                assert_eq!(r1.clean, failure == 0);
            } else {
                assert_eq!(r1.status, "adapter_refused");
                assert!(!r1.clean);
                assert!(value["adapter"]["raw_delta"].is_null());
                assert_eq!(value["aggregate"]["pass_rate"], 0.5);
                assert!(value["adapter"]["r2"]["status"] != "scored");
            }
        }
    }

    #[test]
    fn absent_and_stale_reference_or_stub_refuse() {
        refused(vec![], "missing matching reference");
        let mut rows = records();
        rows.pop();
        refused(rows, "missing matching stub");
        for field in ["oracle", "bundle", "corpus"] {
            let mut rows = records();
            for r in &mut rows[..3] {
                match field {
                    "oracle" => r.oracle_sha = "stale".into(),
                    "bundle" => r.oracle_bundle_sha256 = "stale".into(),
                    _ => r.content_sha256 = "stale".into(),
                }
            }
            refused(rows, "missing matching reference");
            let mut rows = records();
            match field {
                "oracle" => rows[3].oracle_sha = "stale".into(),
                "bundle" => rows[3].oracle_bundle_sha256 = "stale".into(),
                _ => rows[3].content_sha256 = "stale".into(),
            }
            refused(rows, "missing matching stub");
        }
    }
    #[test]
    fn reference_requires_complete_latest_batch_not_a_selection_of_old_passes() {
        let mut rows = records();
        rows.remove(0);
        refused(rows, "incomplete");
        let mut rows = records();
        rows[0].control_repeat = Some(2);
        refused(rows, "repeat identities");
        let mut rows = records();
        rows[0].control_repeats = Some(2);
        refused(rows, "incomplete");
        let mut rows = records();
        rows.push(record(
            "control-reference",
            Uuid::new_v4(),
            1,
            &[("behavior", true), ("hollow", true)],
        ));
        refused(rows, "incomplete");
    }
    #[test]
    fn every_reference_repeat_must_pass_all_checks() {
        for index in 0..3 {
            let mut rows = records();
            rows[index].checks[0].passed = false;
            rows[index].aggregate = Some(Aggregate {
                passed: 1,
                total: 2,
                pass_rate: 0.5,
            });
            refused(rows, "every repeat must pass 100%");
        }
    }
    #[test]
    fn contaminated_or_failed_controls_refuse_independently_for_reference_and_stub() {
        for index in [0, 3] {
            for mutation in 0..15 {
                let mut rows = records();
                let r = &mut rows[index];
                match mutation {
                    0 => r.clean = false,
                    1 => r.root_fresh = false,
                    2 => r.unconfined = true,
                    3 => r.guard_hits.push("fallback".into()),
                    4 => r.flags.push("dirty_corpus".into()),
                    5 => r.oracle_exit = Some(1),
                    6 => r.install_exit = Some(2),
                    7 => r.status = "install_failed".into(),
                    8 => r.error = Some("execution error".into()),
                    9 => r.format_version = 2,
                    10 => r.oracle_exit = None,
                    11 => r.survivors_detected = 1,
                    12 => r.guard_bundle_sha256 = None,
                    13 => r.guard_bundle_sha256 = Some("obsolete-guards".into()),
                    _ => r.env_executable = None,
                }
                refused(rows, "clean-sandbox");
            }
        }
        let mut rows = records();
        rows[3].stub_tools = None;
        refused(rows, "generated no-op");
        let mut rows = records();
        rows[3].install_exit = Some(0);
        refused(rows, "project installer");
    }
    #[test]
    fn control_check_sets_and_aggregates_must_match() {
        let mut rows = records();
        rows[0].checks[0].name = "changed".into();
        refused(rows, "check-name set changed");
        let mut rows = records();
        rows[3].checks[0].name = "changed".into();
        refused(rows, "differs from reference");
        let mut rows = records();
        rows[0].aggregate.as_mut().unwrap().passed = 1;
        refused(rows, "aggregate");
        let mut rows = records();
        rows[0].checks[0].name = "hollow".into();
        refused(rows, "duplicate");
    }
    #[test]
    fn empty_discriminating_denominator_refuses() {
        let mut rows = records();
        rows[3].checks[0].passed = true;
        rows[3].aggregate = Some(Aggregate {
            passed: 2,
            total: 2,
            pass_rate: 1.0,
        });
        refused(rows, "empty discriminating denominator");
    }
    #[test]
    fn discriminating_score_excludes_and_lists_stub_passes_but_keeps_raw_r1() {
        let gate = gate(records()).unwrap();
        let mut trial = record(
            "trial",
            Uuid::new_v4(),
            1,
            &[("behavior", false), ("hollow", true)],
        );
        gate.apply(&mut trial).unwrap();
        assert_eq!(trial.aggregate.as_ref().unwrap().pass_rate, 0.5);
        assert_eq!(trial.discriminating.as_ref().unwrap().total, 1);
        assert_eq!(trial.discriminating.as_ref().unwrap().passed, 0);
        assert_eq!(trial.excluded_checks, ["hollow"]);
        assert_eq!(trial.control_ids.len(), 4);
        trial.checks[0].name = "unknown".into();
        assert!(gate
            .apply(&mut trial)
            .unwrap_err()
            .to_string()
            .contains("check-name set"));
    }
    #[test]
    fn ordinary_nonzero_oracle_exit_preserves_flagged_score_after_valid_controls() {
        let gate = gate(records()).unwrap();
        let mut trial = record(
            "trial",
            Uuid::new_v4(),
            1,
            &[("behavior", false), ("hollow", true)],
        );
        trial.oracle_exit = Some(7);
        trial.flags.push("oracle_nonzero_exit".into());
        trial.clean = false;
        gate.apply(&mut trial).unwrap();
        assert_eq!(trial.discriminating.as_ref().unwrap().pass_rate, 0.0);
        assert_eq!(trial.oracle_exit, Some(7));
        assert!(!trial.clean);
        trial.status = "error".into();
        trial.checks.clear();
        trial.aggregate = None;
        assert!(
            gate.apply(&mut trial).is_err(),
            "no checks remains an error"
        );
    }
    #[test]
    fn trusted_config_requires_positive_reference_repeats() {
        let base = "[regen]\noracle_command=['oracle.sh']\noracle_paths=['oracle.sh']\n";
        assert_eq!(RegenConfig::parse(base).unwrap().reference_repeats, 3);
        assert!(RegenConfig::parse(&format!("{base}reference_repeats=0\n")).is_err());
        assert_eq!(
            RegenConfig::parse(&format!("{base}reference_repeats=1\n"))
                .unwrap()
                .reference_repeats,
            1
        );
        assert!(RegenConfig::parse(&format!("{base}reference_repeats=-1\n")).is_err());
    }
    #[test]
    fn cli_requires_exactly_a_trial_directory_or_generated_control() {
        use clap::Parser;
        let suffix = [
            "--corpus",
            "/corpus",
            "--model",
            "m",
            "--vendor",
            "v",
            "--harness",
            "h",
        ];
        for middle in [
            vec!["--control", "reference"],
            vec!["--control", "stub"],
            vec!["/trial"],
        ] {
            let argv: Vec<_> = [vec!["aida", "regen", "score"], middle, suffix.to_vec()].concat();
            assert!(crate::cli::Cli::try_parse_from(argv).is_ok());
        }
        for middle in [
            vec![],
            vec!["--control", "caller"],
            vec!["/caller", "--control", "reference"],
        ] {
            let argv: Vec<_> = [vec!["aida", "regen", "score"], middle, suffix.to_vec()].concat();
            assert!(crate::cli::Cli::try_parse_from(argv).is_err());
        }
    }
    #[test]
    fn controls_reader_uses_committed_objects_and_refuses_corrupt_records() {
        let store = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| super::super::super::git(store.path(), args).unwrap();
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        let rows = records();
        std::fs::create_dir_all(store.path().join("regen/trials")).unwrap();
        for r in &rows {
            std::fs::write(
                store
                    .path()
                    .join(format!("regen/trials/{}.json", r.trial_id)),
                serde_json::to_vec(r).unwrap(),
            )
            .unwrap();
        }
        git(&["add", "."]);
        git(&["commit", "-m", "controls"]);
        // Worktree tampering cannot replace committed control evidence.
        std::fs::write(
            store
                .path()
                .join(format!("regen/trials/{}.json", rows[0].trial_id)),
            b"not json",
        )
        .unwrap();
        assert_eq!(read_records(store.path()).unwrap().len(), 4);
        gate(read_records(store.path()).unwrap()).unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "corrupt control"]);
        assert!(read_records(store.path())
            .unwrap_err()
            .to_string()
            .contains("invalid control/trial"));
    }
    // Exercise the public command path, including its pre-execution load seam.
    // trace:TASK-1731 | ai:codex
    #[test]
    fn handle_score_refuses_absent_or_failed_committed_controls_without_scored_record() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        if !matches!(
            crate::session::bwrap_availability(),
            crate::session::BwrapAvailability::Ok
        ) {
            return;
        }
        let cache = tempfile::tempdir_in("/var/tmp").unwrap();
        let _env =
            crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", cache.path().to_str().unwrap());
        let project = tempfile::tempdir().unwrap();
        let git = |root: &Path, args: &[&str]| super::super::super::git(root, args).unwrap();
        git(project.path(), &["init", "-b", "main"]);
        git(project.path(), &["config", "user.name", "Test"]);
        git(
            project.path(),
            &["config", "user.email", "test@example.com"],
        );
        std::fs::create_dir(project.path().join(".aida")).unwrap();
        std::fs::write(
            project.path().join(".aida/config.toml"),
            "[regen]\noracle_command=['oracle.sh']\noracle_paths=['oracle.sh']\n",
        )
        .unwrap();
        std::fs::write(
            project.path().join("oracle.sh"),
            "#!/bin/sh\nprintf 'ok   - behavior\\nok   - hollow\\n'\n",
        )
        .unwrap();
        std::fs::set_permissions(
            project.path().join("oracle.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        git(project.path(), &["add", "."]);
        git(project.path(), &["commit", "-m", "trusted oracle"]);
        let (config, sha) = load_config(project.path()).unwrap();
        let trial = tempfile::tempdir().unwrap();
        let bundle =
            OracleBundle::freeze(project.path(), sha, &config.oracle_paths, trial.path()).unwrap();
        let guards = ScoreEnv::for_project(&config, false, project.path()).unwrap();
        let corpus = tempfile::tempdir().unwrap();
        let bytes = br#"{"format_version":1,"rows":[]}"#;
        let corpus_hash = format!("{:x}", Sha256::digest(bytes));
        std::fs::write(corpus.path().join("corpus.json"), bytes).unwrap();
        std::fs::write(
            corpus.path().join("manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "format_version": 1, "store_sha": "a".repeat(40),
                "corpus_file": "corpus.json", "content_sha256": corpus_hash,
                "row_count": 0
            }))
            .unwrap(),
        )
        .unwrap();
        for failed_batch in [false, true] {
            let store = project.path().join(".aida-store");
            std::fs::create_dir(&store).unwrap();
            git(&store, &["init", "-b", "aida-store"]);
            git(&store, &["config", "user.name", "Test"]);
            git(&store, &["config", "user.email", "test@example.com"]);
            if failed_batch {
                let mut rows = records();
                rows[0].checks[0].passed = false;
                rows[0].aggregate = Some(Aggregate {
                    passed: 1,
                    total: 2,
                    pass_rate: 0.5,
                });
                std::fs::create_dir_all(store.join("regen/trials")).unwrap();
                for r in &mut rows {
                    r.oracle_sha = bundle.sha.clone();
                    r.oracle_bundle_sha256 = bundle.hash.clone();
                    r.content_sha256 = corpus_hash.clone();
                    r.guard_bundle_sha256 = Some(guards.guards.hash.clone());
                    std::fs::write(
                        store.join(format!("regen/trials/{}.json", r.trial_id)),
                        serde_json::to_vec(r).unwrap(),
                    )
                    .unwrap();
                }
                git(&store, &["add", "."]);
            }
            git(
                &store,
                &["commit", "--allow-empty", "-m", "control fixture"],
            );
            let before_head = super::super::super::head(&store).unwrap();
            let before_tree = git(&store, &["ls-tree", "-r", "HEAD"]);
            let error = handle_score(
                &store,
                Some(trial.path()),
                None,
                corpus.path(),
                "m",
                None,
                "v",
                "h",
                false,
            )
            .unwrap_err();
            let expected = if failed_batch {
                "reference control failed: every repeat must pass 100%"
            } else {
                "missing matching reference control"
            };
            assert!(format!("{error:#}").contains(expected), "{error:#}");
            assert_eq!(super::super::super::head(&store).unwrap(), before_head);
            assert_eq!(git(&store, &["ls-tree", "-r", "HEAD"]), before_tree);
            assert!(git(&store, &["status", "--porcelain", "--untracked-files=all"]).is_empty());
            assert!(read_records(&store)
                .unwrap()
                .iter()
                .all(|r| r.kind != "trial"));
            std::fs::remove_dir_all(&store).unwrap();
        }
    }
    // trace:STORY-1676 | ai:codex
    #[test]
    fn handle_score_persists_installer_failures_and_flagged_family_score_after_real_controls() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        if !matches!(
            crate::session::bwrap_availability(),
            crate::session::BwrapAvailability::Ok
        ) {
            return;
        }
        let cache = tempfile::tempdir_in("/var/tmp").unwrap();
        let _env =
            crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", cache.path().to_str().unwrap());
        let project = tempfile::tempdir().unwrap();
        let git = |root: &Path, args: &[&str]| super::super::super::git(root, args).unwrap();
        git(project.path(), &["init", "-b", "main"]);
        git(project.path(), &["config", "user.name", "Test"]);
        git(
            project.path(),
            &["config", "user.email", "test@example.com"],
        );
        std::fs::create_dir(project.path().join(".aida")).unwrap();
        std::fs::write(project.path().join(".aida/config.toml"), "[regen]\noracle_command=['oracle.sh']\noracle_paths=['oracle.sh']\nrequired_tools=['tool']\nreference_repeats=1\n").unwrap();
        let install = r#"#!/bin/sh
set -eu
test -z "${AIDA_REGEN_CHECK_RESULTS+x}"
printf '#!/bin/sh\nprintf %s\n' "$(cat behavior)" > "$PREFIX/bin/tool"
chmod +x "$PREFIX/bin/tool"
"#;
        let oracle = r#"#!/bin/sh
if test "$("$AIDA_REGEN_PREFIX/bin/tool")" = real; then
 printf 'ok   - behavior\n'
else
 printf 'FAIL - behavior\n'
 if test -f "$AIDA_REGEN_TRIAL_ROOT/crash"; then kind=crash; else kind=assertion; fi
 printf '{"v":1,"check":"behavior","kind":"%s","exit":127}\n' "$kind" > "$AIDA_REGEN_CHECK_RESULTS"
fi
"#;
        for (name, body) in [
            ("install.sh", install),
            ("oracle.sh", oracle),
            ("behavior", "real"),
        ] {
            std::fs::write(project.path().join(name), body).unwrap();
            std::fs::set_permissions(
                project.path().join(name),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        git(project.path(), &["add", "."]);
        git(project.path(), &["commit", "-m", "trusted fixture"]);
        let store = project.path().join(".aida-store");
        std::fs::create_dir(&store).unwrap();
        git(&store, &["init", "-b", "aida-store"]);
        git(&store, &["config", "user.name", "Test"]);
        git(&store, &["config", "user.email", "test@example.com"]);
        git(&store, &["commit", "--allow-empty", "-m", "fixture"]);
        let corpus = tempfile::tempdir().unwrap();
        let bytes = br#"{"format_version":1,"rows":[]}"#;
        std::fs::write(corpus.path().join("corpus.json"), bytes).unwrap();
        std::fs::write(corpus.path().join("manifest.json"), serde_json::to_vec(&serde_json::json!({
            "format_version":1,"store_sha":"a".repeat(40),"content_sha256":format!("{:x}", Sha256::digest(bytes)),
            "corpus_file":"corpus.json","row_count":0
        })).unwrap()).unwrap();
        for control in [
            crate::cli::RegenControl::Reference,
            crate::cli::RegenControl::Stub,
        ] {
            handle_score(
                &store,
                None,
                Some(control),
                corpus.path(),
                "m",
                None,
                "v",
                "h",
                false,
            )
            .unwrap();
        }
        let trial = tempfile::tempdir().unwrap();
        assert!(handle_score(
            &store,
            Some(trial.path()),
            None,
            corpus.path(),
            "m",
            Some("family"),
            "v",
            "h",
            false
        )
        .is_err());
        std::fs::write(trial.path().join("install.sh"), "#!/bin/sh\nexit 8\n").unwrap();
        std::fs::set_permissions(
            trial.path().join("install.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(handle_score(
            &store,
            Some(trial.path()),
            None,
            corpus.path(),
            "m",
            Some("family"),
            "v",
            "h",
            false
        )
        .is_err());
        std::fs::write(trial.path().join("install.sh"), install).unwrap();
        std::fs::write(trial.path().join("behavior"), "bad").unwrap();
        std::fs::write(trial.path().join("crash"), "").unwrap();
        handle_score(
            &store,
            Some(trial.path()),
            None,
            corpus.path(),
            "m",
            Some("family"),
            "v",
            "h",
            false,
        )
        .unwrap();
        let rows = read_records(&store).unwrap();
        assert_eq!(
            rows.iter()
                .filter(|r| r.kind == "trial" && r.status == "install_failed")
                .count(),
            2
        );
        let trial = rows
            .iter()
            .find(|r| r.kind == "trial" && r.status == "scored")
            .unwrap();
        assert_eq!(trial.model_family.as_deref(), Some("family"));
        assert_eq!(trial.discriminating.as_ref().unwrap().pass_rate, 0.0);
        assert!(trial
            .flags
            .iter()
            .any(|f| f == "crash-dominated: review before reporting"));
        assert!(!trial.clean);
        assert_eq!(diagnostics::summary(trial).crashes, 1);
        assert!(rows
            .iter()
            .filter(|r| r.kind.starts_with("control-"))
            .all(|r| r.clean));
        let out = report::read_report(&store).unwrap();
        assert!(out
            .lines()
            .next()
            .unwrap()
            .contains("crash-dominated: review before reporting"));
    }

    #[test]
    fn trial_sandbox_contamination_refuses_even_with_valid_controls() {
        let gate = gate(records()).unwrap();
        let mut trial = record(
            "trial",
            Uuid::new_v4(),
            1,
            &[("behavior", true), ("hollow", true)],
        );
        trial.guard_hits.push("fallback".into());
        assert!(gate
            .apply(&mut trial)
            .unwrap_err()
            .to_string()
            .contains("clean-sandbox"));
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod execution_tests {
    use super::*;
    #[test]
    fn confined_reference_repeats_stub_and_trials_use_independent_roots_and_tools() {
        if !matches!(
            crate::session::bwrap_availability(),
            crate::session::BwrapAvailability::Ok
        ) {
            return;
        }
        let source = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| super::super::super::git(source.path(), args).unwrap();
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        let installer = "#!/bin/sh\nset -eu\ntest ! -e installed.marker\ntouch installed.marker\nprintf '#!/bin/sh\\nprintf real\\n' > \"$PREFIX/bin/regen-fixture-tool\"\nchmod +x \"$PREFIX/bin/regen-fixture-tool\"\n";
        let oracle = "#!/bin/sh\nset -eu\n\"$AIDA_REGEN_PREFIX/bin/regen-fixture-tool\" >/dev/null\nprintf 'ok   - hollow\\n'\nif test \"$(\"$AIDA_REGEN_PREFIX/bin/regen-fixture-tool\")\" = real; then printf 'ok   - behavior\\n'; else printf 'FAIL - behavior\\n'; fi\n";
        for (name, bytes) in [("install.sh", installer), ("oracle.sh", oracle)] {
            std::fs::write(source.path().join(name), bytes).unwrap();
            std::fs::set_permissions(
                source.path().join(name),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        git(&["add", "."]);
        git(&["commit", "-m", "trusted system and oracle"]);
        let sha = String::from_utf8(git(&["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        let cfg = RegenConfig {
            oracle_command: vec!["oracle.sh".into()],
            oracle_paths: vec!["oracle.sh".into()],
            install_command: None,
            required_tools: vec!["regen-fixture-tool".into()],
            reference_repeats: 3,
            crash_failure_share_percent: 50,
        };
        let cache = tempfile::tempdir_in("/var/tmp").unwrap();
        let bundle_trial = fresh::FreshRoot::empty().unwrap();
        let bundle = OracleBundle::freeze_in_cache(
            source.path(),
            sha.clone(),
            &cfg.oracle_paths,
            bundle_trial.path(),
            cache.path(),
        )
        .unwrap();
        let mut records = Vec::new();
        let guard_hash = ScoreEnv::new(&cfg, false).unwrap().guards.hash.clone();
        let run_id = Uuid::new_v4();
        // Dirty source must not influence reference controls.
        std::fs::write(source.path().join("install.sh"), "#!/bin/sh\nexit 88\n").unwrap();
        for repeat in 1..=3 {
            let root = fresh::FreshRoot::reference(source.path(), &sha).unwrap();
            let env = ScoreEnv::new(&cfg, false).unwrap();
            env.require_source_hidden(root.path(), source.path())
                .unwrap();
            root.assert_pristine().unwrap();
            let r = run_trial_in_env(
                &cfg,
                root.path(),
                super::super::tests::manifest(),
                "m",
                "v",
                "h",
                false,
                env,
                &bundle,
                Some(RunSetup {
                    kind: "control-reference",
                    run_id: Some(run_id),
                    repeat: Some(repeat),
                    repeats: Some(3),
                }),
            )
            .unwrap();
            assert_eq!(r.status, "scored", "{:?}", r.error);
            assert!(r.clean && r.root_fresh);
            assert_eq!(r.aggregate.as_ref().unwrap().pass_rate, 1.0);
            assert!(
                !root.path().join("installed.marker").exists(),
                "execution stages inputs without mutating their source"
            );
            records.push(r);
        }
        let root = fresh::FreshRoot::empty().unwrap();
        let mut stub_cfg = cfg.clone();
        stub_cfg.install_command = Some(vec!["/bin/false".into()]);
        let stub = run_trial_in_env(
            &stub_cfg,
            root.path(),
            super::super::tests::manifest(),
            "m",
            "v",
            "h",
            false,
            ScoreEnv::new(&cfg, false).unwrap(),
            &bundle,
            Some(RunSetup {
                kind: "control-stub",
                run_id: Some(Uuid::new_v4()),
                repeat: Some(1),
                repeats: Some(1),
            }),
        )
        .unwrap();
        assert!(stub.clean && stub.root_fresh, "{:?}", stub.error);
        assert_eq!(
            stub.install_exit, None,
            "project installer must never run for stub"
        );
        assert!(stub.guard_hits.is_empty());
        assert_eq!(stub.stub_tools.as_deref(), Some("generated-noop"));
        assert_eq!(
            stub.checks,
            vec![
                CheckOutcome {
                    name: "behavior".into(),
                    passed: false
                },
                CheckOutcome {
                    name: "hollow".into(),
                    passed: true
                }
            ]
        );
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
        records.push(stub);
        let manifest = super::super::tests::manifest();
        let gate = ControlGate::from_records(
            records,
            &bundle.sha,
            &bundle.hash,
            &manifest.content_sha256,
            3,
            &guard_hash,
        )
        .unwrap();
        // Trial snapshots are independent as well: each installer sees no prior marker.
        std::fs::write(source.path().join("install.sh"), installer).unwrap();
        for _ in 0..2 {
            let root = fresh::FreshRoot::trial(source.path()).unwrap();
            let mut r = run_trial_in_env(
                &cfg,
                root.path(),
                manifest.clone(),
                "m",
                "v",
                "h",
                false,
                ScoreEnv::new(&cfg, false).unwrap(),
                &bundle,
                Some(RunSetup {
                    kind: "trial",
                    run_id: None,
                    repeat: None,
                    repeats: None,
                }),
            )
            .unwrap();
            gate.apply(&mut r).unwrap();
            assert_eq!(r.aggregate.as_ref().unwrap().total, 2);
            assert_eq!(r.discriminating.as_ref().unwrap().total, 1);
            assert_eq!(r.excluded_checks, ["hollow"]);
            assert!(!source.path().join("installed.marker").exists());
        }
        // Real trials receive no generated required_tools behind the installer's back.
        let root = fresh::FreshRoot::empty().unwrap();
        let r = run_trial_in_env(
            &cfg,
            root.path(),
            manifest,
            "m",
            "v",
            "h",
            false,
            ScoreEnv::new(&cfg, false).unwrap(),
            &bundle,
            Some(RunSetup {
                kind: "trial",
                run_id: None,
                repeat: None,
                repeats: None,
            }),
        )
        .unwrap();
        assert_eq!(r.status, "error");
        assert!(r.install_exit.is_none());
        assert!(r.guard_hits.is_empty());
        assert!(r.stub_tools.is_none());
    }
}
