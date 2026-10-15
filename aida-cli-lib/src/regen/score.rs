//! Raw R1 scoring with the existing session confinement implementation.
// trace:TASK-1730 | ai:codex

use std::collections::{BTreeMap, HashSet};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

mod oracle; // trace:TASK-1730 | ai:codex
use oracle::OracleBundle;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegenConfig {
    oracle_command: Vec<String>,
    oracle_paths: Vec<String>,
    install_command: Option<Vec<String>>,
    #[serde(default)]
    required_tools: Vec<String>,
}

impl RegenConfig {
    fn parse(body: &str) -> Result<Self> {
        let body: toml::Value = toml::from_str(body).context("invalid trusted project config")?;
        let config: Self = body
            .get("regen")
            .context("trusted project config needs [regen] with oracle_command")?
            .clone()
            .try_into()?;
        validate_argv(&config.oracle_command)?;
        anyhow::ensure!(
            !config.oracle_paths.is_empty(),
            "oracle_paths must not be empty"
        );
        if let Some(argv) = &config.install_command {
            validate_argv(argv)?;
        }
        let mut tools = HashSet::new();
        for tool in &config.required_tools {
            anyhow::ensure!(
                !tool.is_empty()
                    && tool
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-+.".contains(&b))
                    && !matches!(tool.as_str(), "." | ".."),
                "required_tools must contain executable names, not paths: {tool}"
            );
            anyhow::ensure!(tools.insert(tool), "duplicate required tool: {tool}");
        }
        Ok(config)
    }
}

fn load_config(project: &Path) -> Result<(RegenConfig, String)> {
    // Resolve once: config and held-out input bytes must share this commit.
    let sha = crate::trusted_config::trusted_default_branch_sha(project)
        .context("cannot resolve trusted default-branch commit for regen")?;
    let bytes = super::git(project, &["show", &format!("{sha}:.aida/config.toml")])?;
    let body = std::str::from_utf8(&bytes).context("trusted config must be UTF-8")?;
    Ok((RegenConfig::parse(body)?, sha))
}

fn validate_argv(argv: &[String]) -> Result<()> {
    anyhow::ensure!(
        !argv.is_empty() && !argv[0].is_empty() && argv.iter().all(|s| !s.contains('\0')),
        "regen commands must be nonempty argv arrays without NUL bytes"
    );
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct CheckOutcome {
    pub(super) name: String,
    pub(super) passed: bool,
}

/// The marker contract is deliberately line-exact; diagnostics with similar
/// prefixes are never counted as checks. Name identity is case-sensitive.
pub(super) fn parse_checks(output: &str) -> Result<Vec<CheckOutcome>> {
    let mut checks = BTreeMap::new();
    // split rather than lines: a CR is not part of v1's LF-only marker grammar.
    for line in output.split('\n') {
        let marker = line
            .strip_prefix("ok   - ")
            .map(|n| (n, true))
            .or_else(|| line.strip_prefix("FAIL - ").map(|n| (n, false)));
        let Some((name, passed)) = marker else {
            continue;
        };
        if name.is_empty() || name.trim() != name || name.chars().any(char::is_control) {
            continue;
        }
        anyhow::ensure!(
            checks.insert(name.to_owned(), passed).is_none(),
            "duplicate or conflicting oracle check: {name}"
        );
    }
    anyhow::ensure!(
        !checks.is_empty(),
        "oracle produced no exact v1 check markers"
    );
    Ok(checks
        .into_iter()
        .map(|(name, passed)| CheckOutcome { name, passed })
        .collect())
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Aggregate {
    pub(super) passed: usize,
    pub(super) total: usize,
    pub(super) pass_rate: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct TrialRecord {
    pub(super) format_version: u32,
    pub(super) trial_id: Uuid,
    pub(super) content_sha256: String,
    pub(super) store_sha: String,
    pub(super) oracle_sha: String,
    pub(super) oracle_bundle_sha256: String,
    pub(super) model: String,
    pub(super) vendor: String,
    pub(super) harness: String,
    pub(super) started_at: DateTime<Utc>,
    pub(super) completed_at: DateTime<Utc>,
    pub(super) checks: Vec<CheckOutcome>,
    pub(super) aggregate: Option<Aggregate>,
    pub(super) install_exit: Option<i32>,
    pub(super) oracle_exit: Option<i32>,
    pub(super) status: String,
    pub(super) clean: bool,
    pub(super) guard_hits: Vec<String>,
    pub(super) unconfined: bool,
    pub(super) flags: Vec<String>,
    pub(super) error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    format_version: u32,
    store_sha: String,
    corpus_file: String,
    content_sha256: String,
    row_count: usize,
    #[serde(default)]
    dirty: bool,
}

fn read_manifest(corpus: &Path) -> Result<Manifest> {
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(corpus.join("manifest.json"))?)
        .context("invalid corpus manifest")?;
    anyhow::ensure!(
        manifest.format_version == 1 && manifest.corpus_file == "corpus.json",
        "unsupported corpus manifest format"
    );
    anyhow::ensure!(
        matches!(manifest.store_sha.len(), 40 | 64)
            && manifest.store_sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid manifest store SHA"
    );
    let bytes = std::fs::read(corpus.join("corpus.json"))?;
    anyhow::ensure!(
        manifest.content_sha256 == format!("{:x}", Sha256::digest(&bytes)),
        "corpus content hash mismatch"
    );
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        value["format_version"] == 1
            && value["rows"]
                .as_array()
                .is_some_and(|r| r.len() == manifest.row_count),
        "corpus format or row count mismatch"
    );
    Ok(manifest)
}

/// Scoring scratch is the only additional writable path passed to the existing
/// bwrap helper. HOME/XDG/PREFIX and the fallback guard all live here.
struct ScoreEnv {
    root: PathBuf,
    prefix: PathBuf,
    guard_log: PathBuf,
    path: std::ffi::OsString,
    bwrap: Option<PathBuf>,
}

impl Drop for ScoreEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl ScoreEnv {
    fn new(config: &RegenConfig, unconfined: bool) -> Result<Self> {
        let bwrap = if unconfined {
            None
        } else {
            // bwrap_availability is the existing public wrapper around the
            // private bwrap_preflight. No duplicate sandbox or preflight.
            match crate::session::bwrap_availability() {
                crate::session::BwrapAvailability::Ok => {},
                availability => anyhow::bail!("bubblewrap unavailable ({availability:?}); scoring refused; --unconfined explicitly records a non-clean trial"),
            }
            Some(
                crate::doctor_root::which_binary("bwrap")
                    .context("bubblewrap vanished after preflight")?
                    .canonicalize()?,
            )
        };
        let root = std::env::temp_dir().join(format!("aida-regen-score-{}", Uuid::new_v4()));
        std::fs::DirBuilder::new().mode(0o700).create(&root)?;
        let mut env = Self {
            prefix: root.join("prefix"),
            guard_log: root.join("guard-hits"),
            root,
            path: Default::default(),
            bwrap,
        };
        for dir in [
            "home",
            "config",
            "data",
            "cache",
            "state",
            "run",
            "guard",
            "prefix/bin",
        ] {
            std::fs::create_dir_all(env.root.join(dir))?;
            std::fs::set_permissions(env.root.join(dir), std::fs::Permissions::from_mode(0o700))?;
        }
        std::fs::write(&env.guard_log, "")?;
        for tool in &config.required_tools {
            let script = format!("#!/bin/sh\nprintf '%s\\n' '{tool}' >> \"$AIDA_REGEN_GUARD_LOG\"\nprintf '%s\\n' 'GUARD: fallback to PATH {tool}' >&2\nexit 97\n");
            let path = env.root.join("guard").join(tool);
            std::fs::write(&path, script)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
        // Prefix tools win; guards intercept any fallback before system dirs.
        env.path = std::env::join_paths([
            env.prefix.join("bin"),
            env.root.join("guard"),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
        ])?;
        Ok(env)
    }

    fn run(&self, dir: &Path, argv: &[String]) -> Result<Output> {
        self.run_with_bundle(dir, argv, None)
    }

    fn run_with_bundle(
        &self,
        dir: &Path,
        argv: &[String],
        bundle: Option<&OracleBundle>,
    ) -> Result<Output> {
        let mut reads = vec![self.root.to_string_lossy().into_owned()];
        if let Some(bundle) = bundle {
            bundle.verify()?;
            reads.push(bundle.root.to_string_lossy().into_owned());
        }
        let mut cmd = if let Some(bwrap) = &self.bwrap {
            let mut cmd = Command::new(bwrap);
            // Reuse the existing strict-read arm, rather than the convenience
            // wrapper's whole-host read-only mount. No sandbox implementation
            // or launcher behavior is changed here.
            cmd.args(crate::session::bwrap_confinement_args_inner(
                dir,
                std::slice::from_ref(&self.root),
                &reads,
            ));
            cmd.arg("--").arg(&argv[0]);
            cmd
        } else {
            Command::new(&argv[0])
        };
        cmd.args(&argv[1..])
            .current_dir(dir)
            .env_clear()
            .env("PATH", &self.path)
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("TMPDIR", &self.root)
            .env("PREFIX", &self.prefix)
            .env("AIDA_REGEN_GUARD_LOG", &self.guard_log)
            .env("AIDA_REGEN_TRIAL_ROOT", dir)
            .env("AIDA_REGEN_PREFIX", &self.prefix)
            .env("LANG", "C.UTF-8")
            .output()
            .with_context(|| format!("run regen command {}", argv[0]))
    }

    fn require_source_hidden(&self, dir: &Path, source: &Path) -> Result<()> {
        if self.bwrap.is_none() {
            return Ok(());
        }
        let probe = self.run(
            dir,
            &[
                "/bin/sh".into(),
                "-c".into(),
                "test ! -e \"$1\"".into(),
                "regen-source-probe".into(),
                // Mounting a nested trial creates empty ancestor directories;
                // the original repository's git entry distinguishes that from
                // exposing the actual source checkout.
                source.join(".git").to_string_lossy().into_owned(),
            ],
        )?;
        anyhow::ensure!(probe.status.success(), "existing confinement exposes the original project at {}; scoring refused (use --unconfined only to record a non-clean trial)", source.display());
        Ok(())
    }

    fn guard_hits(&self) -> Result<Vec<String>> {
        Ok(std::fs::read_to_string(&self.guard_log)?
            .lines()
            .map(str::to_owned)
            .collect())
    }
}

fn execute(
    record: &mut TrialRecord,
    config: &RegenConfig,
    dir: &Path,
    env: &ScoreEnv,
    bundle: &OracleBundle,
) -> Result<()> {
    bundle.verify()?;
    let installer = config.install_command.clone().or_else(|| {
        dir.join("install.sh")
            .is_file()
            .then(|| vec!["./install.sh".into()])
    });
    if let Some(installer) = installer {
        let install = match env.run(dir, &installer) {
            Ok(output) => output,
            Err(error) => {
                record.status = "install_failed".into();
                record.error = Some(format!("installer launch failed: {error:#}"));
                return Ok(());
            }
        };
        record.install_exit = install.status.code();
        if !install.status.success() {
            record.status = "install_failed".into();
            record.error = Some(String::from_utf8_lossy(&install.stderr).into_owned());
            return Ok(());
        }
    }
    let oracle = env
        .run_with_bundle(dir, &bundle.command(&config.oracle_command)?, Some(bundle))
        .context(
            "oracle launch failed; GNU coreutils /usr/bin/env with --chdir support is required",
        )?;
    record.oracle_exit = oracle.status.code();
    if !oracle.status.success() {
        record.flags.push("oracle_nonzero_exit".into());
    }
    record.checks =
        parse_checks(std::str::from_utf8(&oracle.stdout).context("oracle output is not UTF-8")?)
            .with_context(|| {
                format!(
                    "oracle stderr (requires GNU coreutils /usr/bin/env with --chdir support): {}",
                    String::from_utf8_lossy(&oracle.stderr)
                        .chars()
                        .take(4096)
                        .collect::<String>()
                )
            })?;
    let passed = record.checks.iter().filter(|c| c.passed).count();
    let total = record.checks.len();
    record.aggregate = Some(Aggregate {
        passed,
        total,
        pass_rate: passed as f64 / total as f64,
    });
    record.status = "scored".into();
    Ok(())
}

#[cfg(test)]
#[cfg(target_os = "linux")]
fn run_trial(
    config: &RegenConfig,
    dir: &Path,
    manifest: Manifest,
    model: &str,
    vendor: &str,
    harness: &str,
    unconfined: bool,
) -> Result<TrialRecord> {
    // Legacy execution-policy fixtures become an actual committed executable
    // oracle, exercising the same bundle path as production.
    let repo = tempfile::TempDir::new()?;
    super::git(repo.path(), &["init", "-b", "main"])?;
    super::git(repo.path(), &["config", "user.name", "Test"])?;
    super::git(repo.path(), &["config", "user.email", "test@example.com"])?;
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\"'\"'"));
    let script = format!(
        "#!/bin/sh\nexec {}\n",
        config
            .oracle_command
            .iter()
            .map(|s| quote(s))
            .collect::<Vec<_>>()
            .join(" ")
    );
    std::fs::write(repo.path().join("oracle.sh"), script)?;
    std::fs::set_permissions(
        repo.path().join("oracle.sh"),
        std::fs::Permissions::from_mode(0o755),
    )?;
    super::git(repo.path(), &["add", "."])?;
    super::git(repo.path(), &["commit", "-m", "trusted oracle"])?;
    let sha = String::from_utf8(super::git(repo.path(), &["rev-parse", "HEAD"])?)?
        .trim()
        .to_owned();
    let mut config = config.clone();
    config.oracle_command = vec!["oracle.sh".into()];
    config.oracle_paths = vec!["oracle.sh".into()];
    let cache = tempfile::tempdir_in("/var/tmp")?;
    let bundle =
        OracleBundle::freeze_in_cache(repo.path(), sha, &config.oracle_paths, dir, cache.path())?;
    let env = ScoreEnv::new(&config, unconfined)?;
    run_trial_in_env(
        &config, dir, manifest, model, vendor, harness, unconfined, env, &bundle,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_trial_in_env(
    config: &RegenConfig,
    dir: &Path,
    manifest: Manifest,
    model: &str,
    vendor: &str,
    harness: &str,
    unconfined: bool,
    env: ScoreEnv,
    bundle: &OracleBundle,
) -> Result<TrialRecord> {
    let mut flags = Vec::new();
    if unconfined {
        flags.push("unconfined".into());
    }
    if manifest.dirty {
        flags.push("dirty_corpus".into());
    }
    let mut record = TrialRecord {
        format_version: 1,
        trial_id: Uuid::new_v4(),
        content_sha256: manifest.content_sha256,
        store_sha: manifest.store_sha,
        oracle_sha: bundle.sha.clone(),
        oracle_bundle_sha256: bundle.hash.clone(),
        model: model.into(),
        vendor: vendor.into(),
        harness: harness.into(),
        started_at: Utc::now(),
        completed_at: Utc::now(),
        checks: Vec::new(),
        aggregate: None,
        install_exit: None,
        oracle_exit: None,
        status: "error".into(),
        clean: false,
        guard_hits: Vec::new(),
        unconfined,
        flags,
        error: None,
    };
    if let Err(error) = execute(&mut record, config, dir, &env, bundle) {
        record.error = Some(format!("{error:#}"));
    }
    match env.guard_hits() {
        Ok(hits) => record.guard_hits = hits,
        Err(error) => {
            record.status = "error".into();
            record.error = Some(format!("cannot read guard log: {error}"));
        }
    }
    finalize(&mut record);
    Ok(record)
}

fn finalize(record: &mut TrialRecord) {
    if !record.guard_hits.is_empty() && !record.flags.iter().any(|f| f == "guard_hits") {
        record.flags.push("guard_hits".into());
    }
    record.clean = record.status == "scored" && record.flags.is_empty() && !record.unconfined;
    record.completed_at = Utc::now();
}

fn persist(store: &Path, record: &TrialRecord) -> Result<()> {
    aida_core::db::with_store_write_lock(store, || {
        let relative = format!("regen/trials/{}.json", record.trial_id);
        let path = store.join(&relative);
        std::fs::create_dir_all(path.parent().context("trial record needs a parent")?)?;
        let bytes = super::json_bytes(record)?;
        use std::io::Write;
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?
            .write_all(&bytes)?;
        // Stage and commit only this trial, preserving unrelated dirty/staged state.
        aida_core::git_ops::add_all(store, &relative)?;
        aida_core::git_ops::commit_paths(
            store,
            &format!("regen: record trial {}", record.trial_id),
            &[&relative],
        )
    })?;
    crate::maybe_auto_push_store(store, crate::StoreAutoPushMode::PerWrite, "per-write");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_score(
    store: &Path,
    dir: &Path,
    corpus: &Path,
    model: &str,
    vendor: &str,
    harness: &str,
    unconfined: bool,
) -> Result<()> {
    let project = crate::project_root_from_store_hint(store);
    let (config, oracle_sha) = load_config(&project)?;
    let manifest = read_manifest(corpus)?;
    let environment = ScoreEnv::new(&config, unconfined)?;
    let dir = dir.canonicalize()?;
    let bundle = OracleBundle::freeze(&project, oracle_sha, &config.oracle_paths, &dir)?;
    environment.require_source_hidden(&dir, &project.canonicalize()?)?;
    let record = run_trial_in_env(
        &config,
        &dir,
        manifest,
        model,
        vendor,
        harness,
        unconfined,
        environment,
        &bundle,
    )?;
    persist(store, &record)?;
    println!("{}", serde_json::to_string_pretty(&record)?);
    anyhow::ensure!(
        record.status == "scored",
        "trial {} recorded as {}: {}",
        record.trial_id,
        record.status,
        record.error.as_deref().unwrap_or("scoring failed")
    );
    Ok(())
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn config(oracle: &[&str]) -> RegenConfig {
        RegenConfig {
            oracle_command: oracle.iter().map(|s| (*s).into()).collect(),
            oracle_paths: vec!["oracle.sh".into()],
            install_command: None,
            required_tools: vec!["regen-fixture-tool".into()],
        }
    }

    pub(super) fn manifest() -> Manifest {
        Manifest {
            format_version: 1,
            store_sha: "a".repeat(40),
            corpus_file: "corpus.json".into(),
            content_sha256: "b".repeat(64),
            row_count: 0,
            dirty: false,
        }
    }

    #[test]
    fn markers_are_exact_sorted_and_refuse_duplicates_conflicts_and_empty_sets() {
        let checks = parse_checks("ok   - z\nFAIL - a\nokay - loose\nok - loose\nok   - \nok   - trailing \nFAIL - crlf\r\nleading ok   - not-a-marker\n").unwrap();
        assert_eq!(
            checks,
            vec![
                CheckOutcome {
                    name: "a".into(),
                    passed: false
                },
                CheckOutcome {
                    name: "z".into(),
                    passed: true
                }
            ]
        );
        for text in [
            "ok   - x\nok   - x",
            "ok   - x\nFAIL - x",
            "ok - loose\nFAILURE - loose",
            "",
        ] {
            assert!(parse_checks(text).is_err(), "accepted {text:?}");
        }
    }

    #[test]
    fn config_requires_argv_arrays_and_safe_guard_names() {
        assert!(RegenConfig::parse("[regen]\noracle_command = 'sh -c echo'\n").is_err());
        assert!(RegenConfig::parse("[regen]\noracle_command = []\n").is_err());
        assert!(RegenConfig::parse(
            "[regen]\noracle_command = ['sh']\nrequired_tools = ['../tool']\n"
        )
        .is_err());
        assert!(RegenConfig::parse(
            "[regen]\noracle_command = ['sh', '-c', 'echo ok']\noracle_paths = ['oracle.sh']\nrequired_tools = ['tool']\n"
        )
        .is_ok());
    }

    #[test]
    fn execution_has_private_env_and_installer_prefix_and_records_raw_score() {
        let dir = TempDir::new().unwrap();
        let mut cfg = config(&["/bin/sh", "-c", "test \"$HOME\" != /root && test -d \"$HOME\" && test -d \"$XDG_CONFIG_HOME\" && test -d \"$XDG_RUNTIME_DIR\" && test -z \"${CARGO_HOME+x}\" && test -z \"${RUSTUP_HOME+x}\" && regen-fixture-tool && printf 'ok   - private environment\\nFAIL - assertion\\n'"]);
        cfg.install_command = Some(vec!["/bin/sh".into(), "-c".into(), "printf '#!/bin/sh\\nexit 0\\n' > \"$PREFIX/bin/regen-fixture-tool\"; chmod +x \"$PREFIX/bin/regen-fixture-tool\"".into()]);
        let record = run_trial(
            &cfg,
            dir.path(),
            manifest(),
            "model",
            "vendor",
            "harness",
            true,
        )
        .unwrap();
        assert_eq!(record.status, "scored");
        assert_eq!(record.aggregate.as_ref().unwrap().pass_rate, 0.5);
        assert_eq!(record.install_exit, Some(0));
        assert_eq!(record.oracle_exit, Some(0));
        assert_eq!(record.checks.len(), 2);
        assert_eq!(record.model, "model");
        assert!(record.completed_at >= record.started_at);
        assert!(record.guard_hits.is_empty());
        assert!(!record.clean, "explicitly unconfined must be visible");
    }

    #[test]
    fn guard_fallback_is_reported_and_can_never_be_clean_even_with_passing_checks() {
        let dir = TempDir::new().unwrap();
        let cfg = config(&[
            "/bin/sh",
            "-c",
            "regen-fixture-tool >/dev/null 2>&1; printf 'ok   - hollow pass\\n'",
        ]);
        let mut record = run_trial(&cfg, dir.path(), manifest(), "m", "v", "h", true).unwrap();
        assert_eq!(record.guard_hits, ["regen-fixture-tool"]);
        assert!(record.flags.iter().any(|f| f == "guard_hits"));
        assert_eq!(record.aggregate.as_ref().unwrap().pass_rate, 1.0);
        assert!(!record.clean);
        // Exercise the classification without relying on the separate
        // unconfined flag as the reason that clean is false.
        record.unconfined = false;
        record.flags.clear();
        finalize(&mut record);
        assert!(
            !record.clean,
            "guard hits independently forbid a clean result"
        );
        record.guard_hits.clear();
        record.flags.clear();
        finalize(&mut record);
        assert!(
            record.clean,
            "the same passing record is clean without contamination"
        );
    }

    #[test]
    fn exit_policy_distinguishes_installer_failure_oracle_checks_and_scoring_errors() {
        let dir = TempDir::new().unwrap();
        let mut cfg = config(&["/bin/sh", "-c", "printf 'ok   - check\\n'; exit 7"]);
        let record = run_trial(&cfg, dir.path(), manifest(), "m", "v", "h", true).unwrap();
        assert_eq!(record.status, "scored");
        assert_eq!(record.oracle_exit, Some(7));
        assert_eq!(record.aggregate.unwrap().passed, 1);
        assert!(record.flags.iter().any(|f| f == "oracle_nonzero_exit"));
        cfg.install_command = Some(vec!["/bin/sh".into(), "-c".into(), "exit 3".into()]);
        let record = run_trial(&cfg, dir.path(), manifest(), "m", "v", "h", true).unwrap();
        assert_eq!(record.status, "install_failed");
        assert_eq!(record.install_exit, Some(3));
        assert!(record.oracle_exit.is_none() && record.aggregate.is_none());
        cfg.install_command = None;
        cfg.oracle_command = vec![
            "/bin/sh".into(),
            "-c".into(),
            "echo 'ok - loose'; exit 1".into(),
        ];
        let record = run_trial(&cfg, dir.path(), manifest(), "m", "v", "h", true).unwrap();
        assert_eq!(record.status, "error");
        assert!(record.aggregate.is_none());
        assert!(record.error.unwrap().contains("exact v1"));
    }

    #[test]
    fn default_installer_runs_when_present_and_manifest_tampering_refuses() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("install.sh");
        std::fs::write(&path, "#!/bin/sh\nexit 12\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let record = run_trial(
            &config(&["/bin/sh", "-c", "echo 'ok   - never'"]),
            dir.path(),
            manifest(),
            "m",
            "v",
            "h",
            true,
        )
        .unwrap();
        assert_eq!(record.status, "install_failed");
        assert_eq!(record.install_exit, Some(12));
        let corpus = dir.path().join("corpus");
        std::fs::create_dir(&corpus).unwrap();
        let bytes = b"{\"format_version\":1,\"rows\":[]}\n";
        std::fs::write(corpus.join("corpus.json"), bytes).unwrap();
        let value = serde_json::json!({"format_version":1,"store_sha":"a".repeat(40),"corpus_file":"corpus.json","content_sha256":format!("{:x}", Sha256::digest(bytes)),"row_count":0});
        std::fs::write(
            corpus.join("manifest.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert!(read_manifest(&corpus).is_ok());
        std::fs::write(corpus.join("corpus.json"), "tampered").unwrap();
        assert!(read_manifest(&corpus)
            .unwrap_err()
            .to_string()
            .contains("hash mismatch"));
    }

    #[test]
    fn unavailable_bwrap_refuses_without_explicit_override() {
        if !matches!(
            crate::session::bwrap_availability(),
            crate::session::BwrapAvailability::Ok
        ) {
            assert!(ScoreEnv::new(&config(&["/bin/true"]), false).is_err());
        }
    }

    #[test]
    fn confined_run_uses_existing_helper_when_available() {
        if !matches!(
            crate::session::bwrap_availability(),
            crate::session::BwrapAvailability::Ok
        ) {
            return;
        }
        let dir = TempDir::new().unwrap();
        let cfg = config(&["/bin/sh", "-c", "printf 'ok   - confined\\n'"]);
        let record = run_trial(&cfg, dir.path(), manifest(), "m", "v", "h", false).unwrap();
        assert_eq!(record.status, "scored", "{:?}", record.error);
        assert!(record.clean);
        assert!(!record.unconfined);
    }

    #[test]
    fn confined_environment_hides_original_source_and_refuses_exposed_system_roots() {
        if !matches!(
            crate::session::bwrap_availability(),
            crate::session::BwrapAvailability::Ok
        ) {
            return;
        }
        let dir = TempDir::new().unwrap();
        let cfg = config(&["/bin/true"]);
        let env = ScoreEnv::new(&cfg, false).unwrap();
        env.require_source_hidden(
            dir.path(),
            Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap(),
        )
        .unwrap();
        // The existing helper deliberately exposes essential system paths.
        // A project living there must refuse; it cannot claim clean isolation.
        std::fs::write(dir.path().join(".git"), "visible repository marker").unwrap();
        assert!(env.require_source_hidden(dir.path(), dir.path()).is_err());
        let out = env.run(dir.path(), &["/bin/sh".into(), "-c".into(), "touch \"$HOME/ok\" \"$XDG_CONFIG_HOME/ok\" \"$XDG_DATA_HOME/ok\" \"$XDG_CACHE_HOME/ok\" \"$XDG_STATE_HOME/ok\" \"$XDG_RUNTIME_DIR/ok\"; test -f \"$XDG_RUNTIME_DIR/ok\"".into()]).unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn config_and_oracle_share_pinned_default_branch_commit_not_trial_branch() {
        let repo = TempDir::new().unwrap();
        let git = |args: &[&str]| super::super::git(repo.path(), args).unwrap();
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        std::fs::create_dir(repo.path().join(".aida")).unwrap();
        let path = repo.path().join(".aida/config.toml");
        std::fs::write(
            &path,
            "[regen]\noracle_command = ['oracle.sh']\noracle_paths = ['oracle.sh']\n",
        )
        .unwrap();
        git(&["add", "."]);
        std::fs::write(repo.path().join("oracle.sh"), "trusted bytes").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "trusted config"]);
        git(&["checkout", "-b", "trial"]);
        std::fs::write(path, "[regen]\noracle_command = ['/bin/false']\n").unwrap();
        let (cfg, sha) = load_config(repo.path()).unwrap();
        assert_eq!(cfg.oracle_command, ["oracle.sh"]);
        std::fs::write(repo.path().join("oracle.sh"), "trial replacement").unwrap();
        // Even if the default branch advances after config load, the bundle
        // stays pinned to the config's commit.
        git(&["add", "."]);
        git(&["commit", "-m", "trial config and oracle"]);
        git(&["branch", "-f", "main", "HEAD"]);
        let trial = TempDir::new().unwrap();
        let cache = tempfile::tempdir_in("/var/tmp").unwrap();
        let bundle = OracleBundle::freeze_in_cache(
            repo.path(),
            sha.clone(),
            &cfg.oracle_paths,
            trial.path(),
            cache.path(),
        )
        .unwrap();
        assert_eq!(bundle.sha, sha);
        assert_eq!(
            std::fs::read(bundle.root.join("oracle.sh")).unwrap(),
            b"trusted bytes"
        );
    }

    #[test]
    fn trial_persistence_keeps_unrelated_staged_edits_and_roundtrips_checks() {
        let store = TempDir::new().unwrap();
        super::super::git(store.path(), &["init", "-b", "main"]).unwrap();
        super::super::git(store.path(), &["config", "user.email", "test@example.com"]).unwrap();
        super::super::git(store.path(), &["config", "user.name", "Test"]).unwrap();
        std::fs::write(store.path().join("unrelated"), "original").unwrap();
        super::super::git(store.path(), &["add", "unrelated"]).unwrap();
        super::super::git(store.path(), &["commit", "-m", "fixture"]).unwrap();
        std::fs::write(store.path().join("unrelated"), "staged edit").unwrap();
        super::super::git(store.path(), &["add", "unrelated"]).unwrap();
        let dir = TempDir::new().unwrap();
        let record = run_trial(
            &config(&["/bin/sh", "-c", "printf 'ok   - persisted\\n'"]),
            dir.path(),
            manifest(),
            "m",
            "v",
            "h",
            true,
        )
        .unwrap();
        persist(store.path(), &record).unwrap();
        let path = format!("regen/trials/{}.json", record.trial_id);
        let bytes = super::super::git(store.path(), &["show", &format!("HEAD:{path}")]).unwrap();
        let read: TrialRecord = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(read.checks, record.checks);
        assert_eq!(read.aggregate.unwrap().pass_rate, 1.0);
        assert_eq!(
            super::super::git(store.path(), &["show", "HEAD:unrelated"]).unwrap(),
            b"original"
        );
        assert_eq!(
            std::fs::read(store.path().join("unrelated")).unwrap(),
            b"staged edit"
        );
    }
}
