//! Strict, data-only seam adapter. Captured values are transient, never serialized.
// trace:TASK-1732 | ai:codex
use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GapsFile {
    format_version: u32,
    #[serde(default)]
    environment: BTreeMap<String, String>,
    #[serde(default)]
    gaps: Vec<Gap>,
}

// The shared v1 schema also accepts the diagnostic rows used by gap reporting.
// Keeping one parser prevents two interpretations of the same trial input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Gap {
    id: String,
    category: Category,
    note: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Category {
    Interface,
    Missing,
    Behavior,
    Other,
}

fn identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    (1..=64).contains(&name.len())
        && bytes
            .next()
            .is_some_and(|b| b == b'_' || b.is_ascii_uppercase())
        && bytes.all(|b| b == b'_' || b.is_ascii_uppercase() || b.is_ascii_digit())
}

fn protected(name: &str) -> bool {
    matches!(
        name,
        "PATH"
            | "HOME"
            | "SHELL"
            | "ENV"
            | "BASH_ENV"
            | "IFS"
            | "TMPDIR"
            | "PWD"
            | "OLDPWD"
            | "LANG"
    ) || [
        "LD_",
        "DYLD_",
        "PYTHON",
        "PERL",
        "RUBY",
        "NODE_",
        "JAVA_",
        "CLASSPATH",
        "GIT_",
        "XDG_",
        "AIDA_",
        "LC_",
    ]
    .iter()
    .any(|p| name.starts_with(p))
}

impl GapsFile {
    fn parse(bytes: &[u8]) -> Result<Self> {
        // Do not include untrusted file lines in parser diagnostics.
        let body =
            std::str::from_utf8(bytes).map_err(|_| anyhow::anyhow!("GAPS.toml must be UTF-8"))?;
        let file: Self = toml::from_str(body)
            .map_err(|_| anyhow::anyhow!("invalid strict GAPS.toml v1 schema"))?;
        anyhow::ensure!(
            file.format_version == 1,
            "unsupported GAPS.toml format_version"
        );
        for (source, target) in &file.environment {
            let mapping = format!("{}->{}", source.escape_debug(), target.escape_debug());
            anyhow::ensure!(identifier(source) && identifier(target), "invalid environment mapping {mapping}: expected uppercase identifiers of 1..64 bytes");
            anyhow::ensure!(
                !protected(source) && !protected(target),
                "protected environment mapping {mapping}"
            );
            anyhow::ensure!(
                !["TOKEN", "SECRET", "PASSWORD", "KEY"]
                    .iter()
                    .any(|p| source.to_ascii_uppercase().contains(p)),
                "sensitive source in environment mapping {mapping}"
            );
        }
        let mut targets = BTreeMap::new();
        for (source, target) in &file.environment {
            if source != target {
                if let Some(other_target) = file.environment.get(target) {
                    anyhow::bail!("conflicting environment mappings {source}->{target} and {target}->{other_target}: target is another source");
                }
            }
            if let Some(other_source) = targets.insert(target, source) {
                anyhow::bail!("conflicting environment mappings {other_source}->{target} and {source}->{target}: shared target");
            }
        }
        validate_gaps(&file.gaps)?;
        Ok(file)
    }
}

// trace:TASK-1733 | ai:codex
fn validate_gaps(gaps: &[Gap]) -> Result<()> {
    let mut ids = HashSet::new();
    for gap in gaps {
        anyhow::ensure!(
            (1..=64).contains(&gap.id.len())
                && gap
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "invalid GAPS.toml gap id"
        );
        anyhow::ensure!(
            ids.insert(&gap.id),
            "duplicate GAPS.toml gap id: {}",
            gap.id
        );
        // Parsed as plain text and a closed category, never interpreted.
        let _ = (&gap.category, &gap.note);
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GapSnapshot {
    format_version: u32,
    input_sha256: String,
    rows: Vec<Gap>,
}

// Diagnostic extensions are deliberately decoded independently of the score.
// There is still only one strict GAPS.toml parser.
// trace:TASK-1733 | ai:codex
pub(super) fn gap_counts(value: &serde_json::Value) -> Option<[usize; 4]> {
    let snapshot: GapSnapshot = serde_json::from_value(value.clone()).ok()?;
    if snapshot.format_version != 1
        || snapshot.input_sha256.len() != 64
        || !snapshot.input_sha256.bytes().all(|b| b.is_ascii_hexdigit())
        || validate_gaps(&snapshot.rows).is_err()
    {
        return None;
    }
    let mut counts = [0; 4];
    for row in snapshot.rows {
        counts[match row.category {
            Category::Interface => 0,
            Category::Missing => 1,
            Category::Behavior => 2,
            Category::Other => 3,
        }] += 1;
    }
    Some(counts)
}

// Intentionally neither Debug nor Serialize: values cannot enter trial JSON.
pub(super) struct CapturedAdapter {
    input_sha256: String,
    gaps: Vec<Gap>,
    mappings: BTreeMap<String, String>,
    sources: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Mapping {
    source: String,
    target: String,
    source_present: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct R1Scores {
    aggregate: Option<Aggregate>,
    discriminating: Option<Aggregate>,
}

/// Additive trial-v1 extension. Top-level checks/aggregates/exits remain R1;
/// r2 contains full fresh-run evidence. Deltas are R2 minus R1 pass fractions.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct AdapterRecord {
    format_version: u32,
    adapter_input_sha256: String,
    mappings: Vec<Mapping>,
    r1: R1Scores,
    pub(super) r2: Box<TrialRecord>,
    raw_delta: Option<f64>,
    discriminating_delta: Option<f64>,
}

impl CapturedAdapter {
    pub(super) fn load(root: &Path) -> Result<Option<Self>> {
        let path = root.join("GAPS.toml");
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).context("read GAPS.toml metadata"),
        };
        anyhow::ensure!(
            metadata.is_file(),
            "GAPS.toml must be a regular trial-root file"
        );
        let bytes = std::fs::read(path).context("read GAPS.toml")?;
        Self::capture(&bytes, |name| std::env::var_os(name)).map(Some)
    }

    fn capture(
        bytes: &[u8],
        mut read: impl FnMut(&str) -> Option<std::ffi::OsString>,
    ) -> Result<Self> {
        let file = GapsFile::parse(bytes)?;
        let mut sources = BTreeMap::new();
        // Entire mapping has been validated before the first caller-env read.
        for (source, target) in &file.environment {
            let mapping = format!("{}->{}", source.escape_debug(), target.escape_debug());
            let value = read(source)
                .with_context(|| format!("missing source for environment mapping {mapping}"))?;
            let value = value.into_string().map_err(|_| {
                anyhow::anyhow!("non-UTF-8 value for environment mapping {mapping}")
            })?;
            anyhow::ensure!(value.len() <= 4096 && !value.contains(['\0', '\n', '\r']), "invalid value for environment mapping {mapping}: expected <=4 KiB without NUL or newline");
            sources.insert(source.clone(), value);
        }
        // Stable IDs are persisted diagnostics. Apply the same privacy boundary
        // as oracle check names before any execution; do not echo the ID/value.
        // trace:TASK-1733 | ai:codex
        anyhow::ensure!(
            !sources
                .values()
                .filter(|v| !v.is_empty())
                .any(|value| file.gaps.iter().any(|gap| gap.id.contains(value))),
            "gap identity contains a mapped input value; diagnostics withheld"
        );
        Ok(Self {
            input_sha256: format!("{:x}", Sha256::digest(bytes)),
            gaps: file.gaps,
            mappings: file.environment,
            sources,
        })
    }

    pub(super) fn raw_environment(&self) -> BTreeMap<String, String> {
        self.sources.clone()
    }

    // trace:TASK-1733 | ai:codex
    pub(super) fn gap_snapshot(&self) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "format_version": 1,
            "input_sha256": self.input_sha256,
            // Notes are accepted as plain input text but need not enter durable
            // diagnostics: they could echo captured values or private prose.
            "rows": self.gaps.iter().map(|gap| serde_json::json!({
                "id": gap.id,
                "category": gap.category,
            })).collect::<Vec<_>>(),
        }))
    }

    pub(super) fn adapted_environment(&self) -> BTreeMap<String, String> {
        let mut env = self.raw_environment();
        for (source, target) in &self.mappings {
            env.insert(target.clone(), self.sources[source].clone());
        }
        env
    }

    pub(super) fn attach(self, r1: &mut TrialRecord, r2: TrialRecord) {
        let same_names = r1
            .checks
            .iter()
            .map(|c| &c.name)
            .eq(r2.checks.iter().map(|c| &c.name));
        let comparable = r1.status == "scored" && r2.status == "scored" && same_names;
        let delta = |a: &Option<Aggregate>, b: &Option<Aggregate>| {
            comparable
                .then(|| {
                    a.as_ref()
                        .zip(b.as_ref())
                        .map(|(a, b)| b.pass_rate - a.pass_rate)
                })
                .flatten()
        };
        let raw_delta = delta(&r1.aggregate, &r2.aggregate);
        let discriminating_delta = delta(&r1.discriminating, &r2.discriminating);
        if r1.status == "scored" && !comparable {
            r1.status = "adapter_refused".into();
            r1.error = Some(if r2.status != "scored" {
                format!("R2 recorded as {} (see adapter.r2 evidence)", r2.status)
            } else {
                "R1/R2 oracle check-name sets differ".into()
            });
        }
        r1.clean = r1.clean && r2.clean && comparable;
        r1.completed_at = r2.completed_at;
        r1.adapter = Some(AdapterRecord {
            format_version: 1,
            adapter_input_sha256: self.input_sha256,
            mappings: self
                .mappings
                .into_iter()
                .map(|(source, target)| Mapping {
                    source,
                    target,
                    source_present: true,
                })
                .collect(),
            r1: R1Scores {
                aggregate: r1.aggregate.clone(),
                discriminating: r1.discriminating.clone(),
            },
            r2: Box::new(r2),
            raw_delta,
            discriminating_delta,
        });
    }
}

pub(super) fn apply_gate(record: &mut TrialRecord, gate: Option<&controls::ControlGate>) {
    if record.status == "scored" {
        if let Some(gate) = gate {
            if let Err(error) = gate.apply(record) {
                record.status = "controls_refused".into();
                record.clean = false;
                record.error = Some(format!("{error:#}"));
            }
        }
    }
}

/// Child diagnostics are untrusted: a tool may echo a captured value. Redact
/// stderr errors; refuse value-bearing check identities instead of storing them.
pub(super) fn sanitize_output(record: &mut TrialRecord, env: &BTreeMap<String, String>) {
    if !env.is_empty() && record.error.is_some() {
        // Withhold the entire diagnostic: truncation or encoding could otherwise
        // leave a fragment of an echoed captured value. Status/exits survive.
        record.error = Some(
            if record.status == "install_failed" {
                "installer failed; child diagnostics withheld for mapped input privacy"
            } else {
                "scoring failed; child diagnostics withheld for mapped input privacy"
            }
            .into(),
        );
    }
    for value in env.values().filter(|v| !v.is_empty()) {
        if record.checks.iter().any(|c| c.name.contains(value)) {
            record.checks.clear();
            record.aggregate = None;
            record.status = "error".into();
            record.error =
                Some("oracle check identity contains a mapped input value; output withheld".into());
        }
    }
}

// R1 has finished. R2 gets a separate run root, prefix, install and environment,
// while the private input snapshot and exact frozen oracle remain unchanged.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_adapted(
    r1: &mut TrialRecord,
    adapter: CapturedAdapter,
    root: &fresh::FreshRoot,
    config: &RegenConfig,
    manifest: Manifest,
    project: &Path,
    bundle: &OracleBundle,
    gate: Option<&controls::ControlGate>,
) -> Result<()> {
    let mut failed = initial_record(
        manifest.clone(),
        &r1.model,
        &r1.vendor,
        &r1.harness,
        r1.unconfined,
        bundle,
        None,
    );
    failed.kind = "trial-adapted".into();
    failed.status = "setup_failed".into();
    let mut setup_phase = "input validation";
    let mut r2 = match (|| -> Result<TrialRecord> {
        root.assert_pristine()?;
        setup_phase = "environment creation";
        let mut env = ScoreEnv::for_project(config, r1.unconfined, project)?;
        env.oracle_environment = adapter.adapted_environment();
        setup_phase = "source visibility probe";
        env.require_source_hidden(root.path(), project)?;
        setup_phase = "trial staging";
        run_trial_in_env(
            config,
            root.path(),
            manifest,
            &r1.model,
            &r1.vendor,
            &r1.harness,
            r1.unconfined,
            env,
            bundle,
            Some(controls::RunSetup {
                kind: "trial-adapted",
                run_id: None,
                repeat: None,
                repeats: None,
            }),
        )
    })() {
        Ok(record) => record,
        Err(_) => {
            failed.error = Some(format!("R2 setup failed during {setup_phase}; diagnostics withheld for mapped input privacy"));
            failed.completed_at = Utc::now();
            failed
        }
    };
    apply_gate(&mut r2, gate);
    adapter.attach(r1, r2);
    Ok(())
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn input(mappings: &str) -> String {
        format!("format_version = 1\n[environment]\n{mappings}\n")
    }

    #[test]
    fn strict_shared_schema_and_full_mapping_validation_before_capture() {
        for body in [
            "format_version = 2".to_owned(),
            "format_version = 1\nunknown = true".to_owned(),
            input("bad = 'GOOD'"),
            input("GOOD = 'Bad'"),
            input("GOOD = 'AIDA_REGEN_PREFIX'"),
            input("AIDA_REGEN_TRIAL_ROOT = 'PATH'"),
            input("AIDA_REGEN_TRIAL_ROOT = 'PYTHONPATH'"),
            input("GOOD = 'PATH'"),
            input("PATH = 'GOOD'"),
            input("GOOD = 'XDG_DATA_HOME'"),
            input("SECRET_THING = 'GOOD'"),
            input("MY_KEY = 'GOOD'"),
            input("PASSWORD = 'GOOD'"),
            input("ACCESS_TOKEN = 'GOOD'"),
            input("AAA = 'GOOD'\nZZZ = 'PYTHONPATH'"),
            "format_version=1\n[[gaps]]\nid='x'\ncategory='unknown'".to_owned(),
            "format_version=1\n[[gaps]]\nid='Bad'\ncategory='other'".to_owned(),
            "format_version=1\n[[gaps]]\nid='x'\ncategory='other'\ncommand='sh'".to_owned(),
            "format_version=1\n[[gaps]]\nid='x'\ncategory='other'\n[[gaps]]\nid='x'\ncategory='missing'".to_owned(),
        ] {
            let mut reads = 0;
            assert!(CapturedAdapter::capture(body.as_bytes(), |_| { reads += 1; Some("not read".into()) }).is_err(), "accepted {body}");
            assert_eq!(reads, 0, "captured before validation: {body}");
        }
        assert!(GapsFile::parse(b"format_version=1\n[[gaps]]\nid='seam-1'\ncategory='interface'\nnote='$(never executed)'\n[environment]\nOLD='NEW'").is_ok());
        for target in [
            "HOME",
            "SHELL",
            "ENV",
            "BASH_ENV",
            "IFS",
            "TMPDIR",
            "PWD",
            "OLDPWD",
            "LANG",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "PYTHONPATH",
            "PERL5LIB",
            "RUBYOPT",
            "NODE_OPTIONS",
            "JAVA_HOME",
            "CLASSPATH",
            "GIT_CONFIG",
            "LC_ALL",
            "AIDA_REGEN_TRIAL_ROOT",
        ] {
            assert!(GapsFile::parse(input(&format!("GOOD='{target}'")).as_bytes()).is_err());
            assert!(GapsFile::parse(input(&format!("{target}='GOOD'")).as_bytes()).is_err());
        }
    }

    // trace:TASK-1733 | ai:codex
    #[test]
    fn diagnostic_snapshot_omits_notes_and_refuses_value_bearing_ids() {
        let value = "private-value";
        let body = "format_version=1\n[environment]\nMODE='RENAMED'\n[[gaps]]\nid='safe-id'\ncategory='interface'\nnote='private-value'\n";
        let captured = CapturedAdapter::capture(body.as_bytes(), |_| Some(value.into())).unwrap();
        let snapshot = captured.gap_snapshot().unwrap();
        assert!(!snapshot.to_string().contains(value));
        assert!(snapshot["rows"][0].get("note").is_none());
        assert_eq!(
            snapshot["input_sha256"],
            format!("{:x}", Sha256::digest(body.as_bytes()))
        );
        assert_eq!(gap_counts(&snapshot), Some([1, 0, 0, 0]));
        let bad = body.replace("safe-id", value);
        let error = match CapturedAdapter::capture(bad.as_bytes(), |_| Some(value.into())) {
            Ok(_) => panic!("persistable identity contains a mapped value"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("gap identity contains a mapped input value"));
        assert!(!error.contains(value));
    }

    #[test]
    fn collisions_refuse_before_capture_and_self_maps_are_recorded_noops() {
        for (map, a, b) in [
            ("A='B'\nB='C'", "A->B", "B->C"),
            ("A='C'\nB='C'", "A->C", "B->C"),
        ] {
            let error = match CapturedAdapter::capture(input(map).as_bytes(), |_| {
                panic!("read before validation")
            }) {
                Err(e) => e.to_string(),
                Ok(_) => panic!("accepted collision"),
            };
            assert!(error.contains(a) && error.contains(b), "{error}");
        }
        let capture =
            CapturedAdapter::capture(input("A='A'").as_bytes(), |_| Some("unchanged".into()))
                .unwrap();
        assert_eq!(capture.raw_environment(), capture.adapted_environment());
        assert_eq!(capture.mappings["A"], "A");
    }

    #[test]
    fn capture_once_verbatim_with_named_errors_and_no_values_in_metadata() {
        use std::os::unix::ffi::OsStringExt;
        let body = input("OLD='NEW'");
        let value = "literal $(echo no); ~/$HOME";
        let mut reads = vec![];
        let capture = CapturedAdapter::capture(body.as_bytes(), |name| {
            reads.push(name.to_owned());
            Some(value.into())
        })
        .unwrap();
        assert_eq!(reads, ["OLD"]);
        assert_eq!(
            capture.raw_environment(),
            BTreeMap::from([("OLD".into(), value.into())])
        );
        assert_eq!(
            capture.adapted_environment(),
            BTreeMap::from([("OLD".into(), value.into()), ("NEW".into(), value.into())])
        );
        assert_eq!(
            capture.input_sha256,
            format!("{:x}", Sha256::digest(body.as_bytes()))
        );
        for invalid in [
            None,
            Some(OsString::from_vec(vec![0xff])),
            Some("a\0b".into()),
            Some("a\nb".into()),
            Some("a\rb".into()),
            Some("q".repeat(4097).into()),
        ] {
            let error = match CapturedAdapter::capture(body.as_bytes(), |_| invalid.clone()) {
                Err(e) => e.to_string(),
                Ok(_) => panic!("accepted invalid capture"),
            };
            assert!(error.contains("OLD->NEW"), "{error}");
            assert!(!error.contains("qqqq"));
        }
        for valid in ["".to_owned(), "q".repeat(4096)] {
            assert!(
                CapturedAdapter::capture(body.as_bytes(), |_| Some(valid.clone().into())).is_ok()
            );
        }
    }

    #[test]
    fn optional_input_requires_regular_file_and_invalid_metadata_does_not_capture() {
        // Survivor fd scans must not observe unrelated libtest forks.
        // trace:STORY-1677 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        assert!(CapturedAdapter::load(root.path()).unwrap().is_none());
        std::fs::write(root.path().join("other"), "format_version=1").unwrap();
        std::os::unix::fs::symlink("other", root.path().join("GAPS.toml")).unwrap();
        assert!(CapturedAdapter::load(root.path()).is_err());
        std::fs::remove_file(root.path().join("GAPS.toml")).unwrap();
        std::fs::create_dir(root.path().join("GAPS.toml")).unwrap();
        assert!(CapturedAdapter::load(root.path()).is_err());
    }

    fn pair(oracle: &str, installer: &str, confined: bool) -> TrialRecord {
        pair_with_setup(oracle, installer, confined, false)
    }

    fn pair_with_setup(oracle: &str, installer: &str, confined: bool, tamper: bool) -> TrialRecord {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| super::super::super::git(repo.path(), args).unwrap();
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        std::fs::write(
            repo.path().join("oracle.sh"),
            format!("#!/bin/sh\n{oracle}\n"),
        )
        .unwrap();
        std::fs::set_permissions(
            repo.path().join("oracle.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "immutable fixture"]);
        let sha = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap();
        let source = tempfile::tempdir().unwrap();
        std::fs::write(
            source.path().join("GAPS.toml"),
            input("CONTRACT_NAME='REGENERATED_NAME'"),
        )
        .unwrap();
        let root = fresh::FreshRoot::trial(source.path()).unwrap();
        let config = RegenConfig {
            oracle_command: vec!["oracle.sh".into()],
            oracle_paths: vec!["oracle.sh".into()],
            reference_repeats: 1,
            min_bytes: 64,
            components: BTreeMap::new(),
            required_tools: vec!["regen-fixture-tool".into()],
            install_command: Some(vec!["/bin/sh".into(), "-c".into(), installer.into()]),
        };
        let cache = tempfile::tempdir_in("/var/tmp").unwrap();
        let bundle = OracleBundle::freeze_in_cache(
            repo.path(),
            sha.trim().into(),
            &config.oracle_paths,
            root.path(),
            cache.path(),
        )
        .unwrap();
        let capture = CapturedAdapter::capture(
            &std::fs::read(root.path().join("GAPS.toml")).unwrap(),
            |_| Some("literal-interface-value".into()),
        )
        .unwrap();
        let mut env = ScoreEnv::for_project(&config, !confined, repo.path()).unwrap();
        env.oracle_environment = capture.raw_environment();
        let mut r1 = run_trial_in_env(
            &config,
            root.path(),
            super::super::tests::manifest(),
            "m",
            "v",
            "h",
            !confined,
            env,
            &bundle,
            Some(controls::RunSetup {
                kind: "trial",
                run_id: None,
                repeat: None,
                repeats: None,
            }),
        )
        .unwrap();
        assert!(r1.adapter.is_none());
        if tamper {
            std::fs::write(root.path().join("unexpected"), "changed input").unwrap();
        }
        // Exact production helper, not a second implementation of R2.
        run_adapted(
            &mut r1,
            capture,
            &root,
            &config,
            super::super::tests::manifest(),
            repo.path(),
            &bundle,
            None,
        )
        .unwrap();
        if !tamper {
            root.assert_pristine().unwrap();
        }
        assert_eq!(std::fs::read_dir(source.path()).unwrap().count(), 1);
        r1
    }

    const INSTALL: &str = "grep 'format_version = 1' GAPS.toml >/dev/null && test -z \"${CONTRACT_NAME+x}\" && test -z \"${REGENERATED_NAME+x}\" && test ! -e installed && test ! -e \"$PREFIX/installed\" && : > installed && : > \"$PREFIX/installed\" && printf bad > GAPS.toml";
    const RENAMED: &str = "test -f \"$AIDA_REGEN_TRIAL_ROOT/installed\" && test -f \"$AIDA_REGEN_PREFIX/installed\" || exit 19; test \"$CONTRACT_NAME\" = 'literal-interface-value' || exit 20; if test \"${REGENERATED_NAME-}\" = \"$CONTRACT_NAME\"; then printf 'ok   - renamed interface\\n'; else printf 'FAIL - renamed interface\\n'; exit 7; fi";

    #[test]
    fn renamed_env_flips_r1_to_r2_with_fresh_installs_and_full_evidence() {
        // Survivor fd scans must not observe unrelated libtest forks.
        // trace:STORY-1677 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let record = pair(RENAMED, INSTALL, false);
        assert_eq!(record.status, "scored", "{:?}", record.error);
        assert_eq!(record.aggregate.as_ref().unwrap().pass_rate, 0.0);
        assert_eq!(record.oracle_exit, Some(7));
        assert!(record.flags.iter().any(|f| f == "oracle_nonzero_exit"));
        let adapter = record.adapter.as_ref().unwrap();
        assert_eq!(adapter.r2.aggregate.as_ref().unwrap().pass_rate, 1.0);
        assert_eq!(adapter.raw_delta, Some(1.0));
        assert_eq!(adapter.r2.install_exit, Some(0));
        assert_eq!(adapter.r2.oracle_exit, Some(0));
        assert_eq!(record.oracle_bundle_sha256, adapter.r2.oracle_bundle_sha256);
        assert_ne!(record.trial_id, adapter.r2.trial_id);
        assert!(adapter.r2.started_at >= record.started_at);
        assert!(adapter.r2.root_fresh);
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("literal-interface-value"));
        let read: TrialRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(read.adapter.unwrap().raw_delta, Some(1.0));
    }

    #[test]
    fn confined_pair_uses_trusted_path_and_fresh_environment() {
        // Survivor fd scans must not observe unrelated libtest forks.
        // trace:STORY-1677 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        if !matches!(
            crate::session::bwrap_availability(),
            crate::session::BwrapAvailability::Ok
        ) {
            return;
        }
        let record = pair(RENAMED, INSTALL, true);
        assert_eq!(record.status, "scored", "{:?}", record.error);
        let adapter = record.adapter.unwrap();
        assert!(adapter.r2.clean, "{:?}", adapter.r2.flags);
        assert_eq!(adapter.raw_delta, Some(1.0));
        // R1's nonzero exit remains visible rather than being overwritten by R2.
        assert!(!record.clean);
    }

    #[test]
    fn r2_mismatch_errors_and_contamination_cannot_hide_behind_passing_r1() {
        // Survivor fd scans must not observe unrelated libtest forks.
        // trace:STORY-1677 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        for oracle in [
            "if test -n \"${REGENERATED_NAME-}\"; then printf 'ok   - other\\n'; else printf 'ok   - original\\n'; fi",
            "if test -n \"${REGENERATED_NAME-}\"; then echo 'ok - loose'; exit 4; else printf 'ok   - original\\n'; fi",
            "if test -n \"${REGENERATED_NAME-}\"; then regen-fixture-tool >/dev/null 2>&1; fi; printf 'ok   - original\\n'",
        ] {
            let record = pair(oracle, INSTALL, false);
            let adapter = record.adapter.as_ref().unwrap();
            assert_eq!(record.aggregate.as_ref().unwrap().pass_rate, 1.0);
            if !adapter.r2.guard_hits.is_empty() {
                assert!(!record.clean);
                assert_eq!(adapter.r2.guard_hits, ["regen-fixture-tool"]);
            } else {
                assert_eq!(record.status, "adapter_refused");
                assert!(adapter.raw_delta.is_none());
            }
        }
    }

    #[test]
    fn installer_failure_retains_both_runs_and_legacy_record_has_no_adapter() {
        // Survivor fd scans must not observe unrelated libtest forks.
        // trace:STORY-1677 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let record = pair(RENAMED, "exit 13", false);
        assert_eq!(record.status, "install_failed");
        let r2 = &record.adapter.as_ref().unwrap().r2;
        assert_eq!(r2.status, "install_failed");
        assert_eq!(r2.install_exit, Some(13));
        assert!(r2.aggregate.is_none() && r2.oracle_exit.is_none());
        let mut legacy = serde_json::to_value(&record).unwrap();
        legacy.as_object_mut().unwrap().remove("adapter");
        assert!(serde_json::from_value::<TrialRecord>(legacy)
            .unwrap()
            .adapter
            .is_none());
    }

    #[test]
    fn r2_setup_failure_is_durable_without_borrowing_r1_execution_evidence() {
        // Survivor fd scans must not observe unrelated libtest forks.
        // trace:STORY-1677 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let record = pair_with_setup("printf 'ok   - fixed\\n'", INSTALL, false, true);
        assert_eq!(record.status, "adapter_refused");
        assert_eq!(record.aggregate.as_ref().unwrap().pass_rate, 1.0);
        let adapter = record.adapter.as_ref().unwrap();
        let r2 = &adapter.r2;
        assert_eq!(r2.status, "setup_failed");
        assert!(r2.error.as_ref().unwrap().contains("input validation"));
        assert!(r2.checks.is_empty() && r2.aggregate.is_none());
        assert!(r2.install_exit.is_none() && r2.oracle_exit.is_none());
        assert!(r2.guard_bundle_sha256.is_none() && r2.env_executable.is_none());
        assert!(!r2.root_fresh && !r2.clean);
        assert!(adapter.raw_delta.is_none() && adapter.discriminating_delta.is_none());
        assert!(serde_json::to_string(&record).is_ok());
    }

    #[test]
    fn child_output_cannot_persist_mapped_values() {
        // Survivor fd scans must not observe unrelated libtest forks.
        // trace:STORY-1677 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let oracle = "if test -n \"${REGENERATED_NAME-}\"; then printf 'FAIL - %s\\n' \"$REGENERATED_NAME\"; else printf 'ok   - fixed\\n'; fi";
        let record = pair(oracle, INSTALL, false);
        assert_eq!(record.status, "adapter_refused");
        assert!(!serde_json::to_string(&record)
            .unwrap()
            .contains("literal-interface-value"));
        let oracle = "printf '%04090d:%s\\n' 0 \"$CONTRACT_NAME\" >&2; echo 'ok - invalid'; exit 3";
        let record = pair(oracle, INSTALL, false);
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("literal-interface-value"));
        assert!(json.contains("child diagnostics withheld"));
    }
}
