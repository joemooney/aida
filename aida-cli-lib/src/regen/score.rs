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

mod adapter; // trace:TASK-1732 | ai:codex
mod controls; // trace:TASK-1731 | ai:codex
mod diagnostics; // trace:STORY-1676 | ai:codex
mod fresh; // trace:TASK-1731 | ai:codex
mod guards; // trace:TASK-1730 | ai:codex
mod oracle; // trace:TASK-1730 | ai:codex
pub(super) mod report; // trace:TASK-1733 | ai:codex
mod roots; // trace:TASK-1730 | ai:codex
use oracle::OracleBundle;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegenConfig {
    oracle_command: Vec<String>,
    oracle_paths: Vec<String>,
    #[serde(default = "default_reference_repeats")]
    reference_repeats: usize,
    #[serde(default = "diagnostics::default_crash_share")]
    crash_failure_share_percent: u8,
    install_command: Option<Vec<String>>,
    #[serde(default)]
    required_tools: Vec<String>,
}

fn default_reference_repeats() -> usize {
    3
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
            config.crash_failure_share_percent <= 100,
            "crash_failure_share_percent must be 0..100"
        );
        anyhow::ensure!(
            config.reference_repeats >= 1,
            "reference_repeats must be >= 1"
        );
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Aggregate {
    pub(super) passed: usize,
    pub(super) total: usize,
    pub(super) pass_rate: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct TrialRecord {
    pub(super) format_version: u32,
    // Diagnostic metadata is optional on older trials. Decode separately so an
    // invalid diagnostic extension cannot hide an otherwise readable score.
    // trace:TASK-1733 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) gaps: Option<serde_json::Value>,
    // Additive, independently versioned R2 evidence; absent on legacy trials/controls.
    // trace:TASK-1732 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) adapter: Option<adapter::AdapterRecord>,
    // Additive v1 extensions. Older records lack fresh-root/control evidence.
    // trace:TASK-1731 | ai:codex
    #[serde(default = "trial_kind")]
    pub(super) kind: String,
    #[serde(default)]
    pub(super) root_fresh: bool,
    #[serde(default)]
    pub(super) control_run_id: Option<Uuid>,
    #[serde(default)]
    pub(super) control_repeat: Option<usize>,
    #[serde(default)]
    pub(super) control_repeats: Option<usize>,
    #[serde(default)]
    pub(super) stub_tools: Option<String>,
    #[serde(default)]
    pub(super) discriminating: Option<Aggregate>,
    #[serde(default)]
    pub(super) excluded_checks: Vec<String>,
    #[serde(default)]
    pub(super) control_ids: Vec<Uuid>,
    pub(super) trial_id: Uuid,
    pub(super) content_sha256: String,
    pub(super) store_sha: String,
    pub(super) oracle_sha: String,
    pub(super) oracle_bundle_sha256: String,
    pub(super) model: String,
    // Optional on legacy trials; explicit identity, never derived from model/vendor.
    // trace:STORY-1676 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) model_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) failure_diagnostics: Option<serde_json::Value>,
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
    #[serde(default)]
    pub(super) survivors_detected: usize,
    pub(super) flags: Vec<String>,
    pub(super) error: Option<String>,
    #[serde(default)]
    pub(super) env_executable: Option<String>,
    #[serde(default)]
    pub(super) guard_bundle_sha256: Option<String>,
}

fn trial_kind() -> String {
    "trial".into()
}

#[derive(Debug, Clone, Deserialize)]
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

/// Each writable install root is independently bound; the run parent itself is
/// never mounted. This is required for same-path quarantine replacement.
struct ScoreEnv {
    run_root: PathBuf,
    root: PathBuf,
    prefix: PathBuf,
    guards: OracleBundle,
    guard_channel: guards::GuardChannel,
    path: std::ffi::OsString,
    bwrap: Option<PathBuf>,
    install_ran: std::cell::Cell<bool>,
    // Explicitly captured interface inputs, supplied only to the oracle.
    // trace:TASK-1732 | ai:codex
    oracle_environment: BTreeMap<String, String>,
    diagnostic_bytes: std::cell::RefCell<Option<Vec<u8>>>,
    #[cfg(test)]
    _guard_cache: tempfile::TempDir,
}

fn phase_dirs(root: &Path) -> Result<()> {
    for name in ["home", "config", "data", "cache", "state", "run", "tmp"] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join(name))?;
    }
    Ok(())
}

impl Drop for ScoreEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.run_root);
    }
}

impl ScoreEnv {
    #[cfg(test)]
    fn new(config: &RegenConfig, unconfined: bool) -> Result<Self> {
        Self::for_project(config, unconfined, &std::env::current_dir()?)
    }

    fn for_project(config: &RegenConfig, unconfined: bool, project: &Path) -> Result<Self> {
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
        let run_root = PathBuf::from("/tmp").join(format!("aida-regen-score-{}", Uuid::new_v4()));
        std::fs::DirBuilder::new().mode(0o700).create(&run_root)?;
        let root = run_root.join("install-env");
        std::fs::DirBuilder::new().mode(0o700).create(&root)?;
        #[cfg(test)]
        let guard_cache = tempfile::TempDir::new_in("/var/tmp")?;
        #[cfg(test)]
        let guards = OracleBundle::freeze_guards_in_cache(
            project,
            &run_root,
            &config.required_tools,
            guard_cache.path(),
        )?;
        #[cfg(not(test))]
        let guards = OracleBundle::freeze_guards(project, &run_root, &config.required_tools)?;
        let guard_channel =
            guards::GuardChannel::new(guards.root.parent().context("guard cache parent missing")?)?;
        let mut env = Self {
            prefix: run_root.join("prefix"),
            run_root,
            guards,
            guard_channel,
            root,
            path: Default::default(),
            bwrap,
            install_ran: std::cell::Cell::new(false),
            oracle_environment: BTreeMap::new(),
            diagnostic_bytes: Default::default(),
            #[cfg(test)]
            _guard_cache: guard_cache,
        };
        phase_dirs(&env.root)?;
        std::fs::create_dir_all(env.prefix.join("bin"))?;
        // Prefix tools win; guards intercept any fallback before system dirs.
        env.path = std::env::join_paths([
            env.prefix.join("bin"),
            env.guards.root.join("bin"),
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
        let phase_root = if bundle.is_some() {
            // Created only after installation and the snapshot. No installer
            // namespace ever receives this root or a bound ancestor of it.
            let root = self.run_root.join("oracle-env");
            std::fs::DirBuilder::new().mode(0o700).create(&root)?;
            phase_dirs(&root)?;
            root
        } else {
            self.root.clone()
        };
        roots::validate_mounts(&[dir.to_path_buf(), phase_root.clone(), self.prefix.clone()])?;
        self.guards.verify()?;
        let mut reads = vec![
            self.guards.root.to_string_lossy().into_owned(),
            self.guard_channel.root.to_string_lossy().into_owned(),
        ];
        if let Some(bundle) = bundle {
            bundle.verify()?;
            reads.push(bundle.root.to_string_lossy().into_owned());
        }
        // Only the oracle receives this scorer-owned file, created after install.
        // Keep the original descriptor: a replaced pathname is never read back.
        // trace:STORY-1676 | ai:codex
        let sidecar = if bundle.is_some() {
            Some(diagnostics::Sidecar::new(
                &phase_root.join("run/check-results.jsonl"),
            )?)
        } else {
            None
        };
        let mut cmd = if let Some(bwrap) = &self.bwrap {
            let mut cmd = Command::new(bwrap);
            // Reuse the existing strict-read arm, rather than the convenience
            // wrapper's whole-host read-only mount. No sandbox implementation
            // or launcher behavior is changed here.
            cmd.args(crate::session::bwrap_confinement_args_inner(
                dir,
                &[phase_root.clone(), self.prefix.clone()],
                &reads,
            ));
            cmd.arg("--").arg(&argv[0]);
            cmd
        } else {
            Command::new(&argv[0])
        };
        use std::os::unix::process::CommandExt;
        let fd = self.guard_channel.fd();
        unsafe {
            cmd.pre_exec(move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let path = if bundle.is_some() {
            std::env::join_paths([
                self.guards.root.join("bin"),
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
            ])?
        } else {
            self.path.clone()
        };
        cmd.args(&argv[1..])
            .current_dir(dir)
            .env_clear()
            .env("PATH", path)
            .env("HOME", phase_root.join("home"))
            .env("XDG_CONFIG_HOME", phase_root.join("config"))
            .env("XDG_DATA_HOME", phase_root.join("data"))
            .env("XDG_CACHE_HOME", phase_root.join("cache"))
            .env("XDG_STATE_HOME", phase_root.join("state"))
            .env("XDG_RUNTIME_DIR", phase_root.join("run"))
            .env("TMPDIR", phase_root.join("tmp"))
            .env("AIDA_REGEN_GUARD_PIPE", &self.guard_channel.pipe)
            .env("AIDA_REGEN_TRIAL_ROOT", dir)
            .env("LANG", "C.UTF-8");
        if bundle.is_none() {
            cmd.env("PREFIX", &self.prefix);
        } else {
            cmd.envs(&self.oracle_environment);
            cmd.env(
                "AIDA_REGEN_CHECK_RESULTS",
                phase_root.join("run/check-results.jsonl"),
            );
            if self.install_ran.get() {
                cmd.env("AIDA_REGEN_PREFIX", &self.prefix);
            }
        }
        let output = cmd
            .output()
            .with_context(|| format!("run regen command {}", argv[0]))?;
        if let Some(sidecar) = sidecar {
            *self.diagnostic_bytes.borrow_mut() = sidecar.read();
        }
        Ok(output)
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

    fn guard_hits(&mut self) -> Result<Vec<String>> {
        self.guard_channel.collect()
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
    // Generated stubs deliberately have no installer. Every actual system must.
    // trace:STORY-1676 | ai:codex
    if record.kind != "control-stub" {
        let Some(installer) = installer else {
            record.status = "install_failed".into();
            record.error = Some("no declared installer or ./install.sh".into());
            return Ok(());
        };
        env.install_ran.set(true);
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
    // Each root has an independent bind. No mounted parent exposes the new
    // entries to a surviving installer namespace. Keep old roots through scoring.
    record.survivors_detected = roots::survivors(
        &[dir.to_path_buf(), env.prefix.clone(), env.root.clone()],
        Some(env.guard_channel.fd()),
        true,
    )?;
    let _snapshot = roots::Quarantine::snapshot(&[dir.to_path_buf(), env.prefix.clone()])?;
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
    record.failure_diagnostics = Some(serde_json::to_value(diagnostics::capture(
        env.diagnostic_bytes.borrow().as_deref(),
        &record.checks,
        config.crash_failure_share_percent,
    ))?);
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
        &config, dir, manifest, model, vendor, harness, unconfined, env, &bundle, None,
    )
}

// Empty evidence is also used when R2 cannot reach execution. No exits, checks,
// guard attestation or fresh-root claim is borrowed from the completed R1.
// trace:TASK-1732 | ai:codex
fn initial_record(
    manifest: Manifest,
    model: &str,
    vendor: &str,
    harness: &str,
    unconfined: bool,
    bundle: &OracleBundle,
    guard_hash: Option<String>,
) -> TrialRecord {
    let mut flags = Vec::new();
    if unconfined {
        flags.push("unconfined".into());
    }
    if manifest.dirty {
        flags.push("dirty_corpus".into());
    }
    TrialRecord {
        format_version: 1,
        gaps: None,
        adapter: None,
        kind: trial_kind(),
        root_fresh: false,
        control_run_id: None,
        control_repeat: None,
        control_repeats: None,
        stub_tools: None,
        discriminating: None,
        excluded_checks: Vec::new(),
        control_ids: Vec::new(),
        trial_id: Uuid::new_v4(),
        content_sha256: manifest.content_sha256,
        store_sha: manifest.store_sha,
        oracle_sha: bundle.sha.clone(),
        oracle_bundle_sha256: bundle.hash.clone(),
        model: model.into(),
        model_family: None,
        failure_diagnostics: None,
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
        survivors_detected: 0,
        flags,
        error: None,
        env_executable: guard_hash.as_ref().map(|_| "/usr/bin/env".into()),
        guard_bundle_sha256: guard_hash,
    }
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
    mut env: ScoreEnv,
    bundle: &OracleBundle,
    setup: Option<controls::RunSetup>,
) -> Result<TrialRecord> {
    let mut record = initial_record(
        manifest,
        model,
        vendor,
        harness,
        unconfined,
        bundle,
        Some(env.guards.hash.clone()),
    );
    if let Some(setup) = setup {
        record.kind = setup.kind.into();
        record.root_fresh = true;
        record.control_run_id = setup.run_id;
        record.control_repeat = setup.repeat;
        record.control_repeats = setup.repeats;
        if record.kind == "control-stub" {
            record.stub_tools = Some("generated-noop".into());
            env.install_ran.set(true);
            for tool in &config.required_tools {
                let path = env.prefix.join("bin").join(tool);
                std::fs::write(&path, "#!/bin/sh\nexit 0\n")?;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
            }
        }
    }
    // Stage before installation so every installed absolute path lives under
    // private /tmp, outside the helper's essential read mounts.
    let trial = env.run_root.join("trial");
    std::fs::DirBuilder::new().mode(0o700).create(&trial)?;
    roots::copy_tree(dir, &trial)?;
    if let Err(error) = execute(&mut record, config, &trial, &env, bundle) {
        record.error = Some(format!("{error:#}"));
    }
    match env.guard_hits() {
        Ok(hits) => record.guard_hits = hits,
        Err(error) => {
            record.status = "error".into();
            record.error = Some(format!("cannot collect guard evidence: {error}"));
        }
    }
    adapter::sanitize_output(&mut record, &env.oracle_environment);
    finalize(&mut record);
    Ok(record)
}

fn finalize(record: &mut TrialRecord) {
    if record.survivors_detected > 0 && !record.flags.iter().any(|f| f == "installer_survivors") {
        record.flags.push("installer_survivors".into());
    }
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

// trace:TASK-1731 | ai:codex
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_score(
    store: &Path,
    dir: Option<&Path>,
    control: Option<crate::cli::RegenControl>,
    corpus: &Path,
    model: &str,
    model_family: Option<&str>,
    vendor: &str,
    harness: &str,
    unconfined: bool,
) -> Result<()> {
    if let Some(family) = model_family {
        anyhow::ensure!(
            diagnostics::valid_family(family),
            "model family must be nonempty, trimmed, at most 128 bytes, without control characters"
        );
    }
    let project = crate::project_root_from_store_hint(store).canonicalize()?;
    let (config, oracle_sha) = load_config(&project)?;
    let manifest = read_manifest(corpus)?;

    anyhow::ensure!(
        dir.is_some() != control.is_some(),
        "provide a system directory or --control, exclusively"
    );
    let repeats = if control == Some(crate::cli::RegenControl::Reference) {
        config.reference_repeats
    } else {
        1
    };
    let run_id = Uuid::new_v4();
    for repeat in 1..=repeats {
        let root = match control {
            Some(crate::cli::RegenControl::Reference) => {
                fresh::FreshRoot::reference(&project, &oracle_sha)?
            }
            Some(crate::cli::RegenControl::Stub) => fresh::FreshRoot::empty()?,
            None => fresh::FreshRoot::trial(dir.context("missing trial directory")?)?,
        };
        let bundle = OracleBundle::freeze(
            &project,
            oracle_sha.clone(),
            &config.oracle_paths,
            root.path(),
        )?;
        // Full validation precedes capture and all execution, including probes.
        // Read from the private input snapshot once; neither install can change it.
        let adapter = if control.is_none() {
            adapter::CapturedAdapter::load(root.path())?
        } else {
            None
        };
        let mut env = ScoreEnv::for_project(&config, unconfined, &project)?;
        if let Some(adapter) = &adapter {
            env.oracle_environment = adapter.raw_environment();
        }
        let gate = if control.is_none() {
            Some(controls::ControlGate::load(
                store,
                &bundle,
                &manifest,
                config.reference_repeats,
                &env.guards.hash,
            )?)
        } else {
            None
        };
        env.require_source_hidden(root.path(), &project)?;
        root.assert_pristine()?;
        let kind = match control {
            Some(crate::cli::RegenControl::Reference) => "control-reference",
            Some(crate::cli::RegenControl::Stub) => "control-stub",
            None => "trial",
        };
        let mut record = run_trial_in_env(
            &config,
            root.path(),
            manifest.clone(),
            model,
            vendor,
            harness,
            unconfined,
            env,
            &bundle,
            Some(controls::RunSetup {
                kind,
                run_id: control.map(|_| run_id),
                repeat: control.map(|_| repeat),
                repeats: control.map(|_| repeats),
            }),
        )?;
        record.model_family = model_family.map(str::to_owned);
        adapter::apply_gate(&mut record, gate.as_ref());
        if let Some(adapter) = adapter {
            // Capture from the same private, pre-execution input as the adapter.
            // Never reread a caller path after installation/scoring.
            // trace:TASK-1733 | ai:codex
            record.gaps = Some(adapter.gap_snapshot()?);
            adapter::run_adapted(
                &mut record,
                adapter,
                &root,
                &config,
                manifest.clone(),
                &project,
                &bundle,
                gate.as_ref(),
            )?;
        }
        // Quality warnings follow execution/control validation, and never
        // contaminate generated controls or turn a score into gate refusal.
        // trace:STORY-1676 | ai:codex
        diagnostics::flag_trial(&mut record);
        persist(store, &record)?;
        // A refused trial is durable diagnostic evidence, never a reported score.
        anyhow::ensure!(
            record.status == "scored",
            "trial {} recorded as {}: {}",
            record.trial_id,
            record.status,
            record.error.as_deref().unwrap_or("scoring failed")
        );
        println!("{}", serde_json::to_string_pretty(&record)?);
    }
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
            reference_repeats: 3,
            crash_failure_share_percent: 50,
            install_command: Some(vec!["/bin/true".into()]),
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
        let mut cfg = config(&["/bin/sh", "-c", "test \"$HOME\" != /root && test -d \"$HOME\" && test -d \"$XDG_CONFIG_HOME\" && test -d \"$XDG_RUNTIME_DIR\" && test -z \"${CARGO_HOME+x}\" && test -z \"${RUSTUP_HOME+x}\" && \"$AIDA_REGEN_PREFIX/bin/regen-fixture-tool\" && printf 'ok   - private environment\\nFAIL - assertion\\n'"]);
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
    fn oracle_ignores_prefix_shadow_and_uses_fresh_python_home() {
        if !matches!(
            crate::session::bwrap_availability(),
            crate::session::BwrapAvailability::Ok
        ) {
            return;
        }
        let dir = TempDir::new().unwrap();
        let mut cfg = config(&["/bin/sh", "-c", "test \"$(grep sentinel \"$AIDA_REGEN_TRIAL_ROOT/value\")\" = sentinel && /usr/bin/python3 -c \"print('ok   - fresh home')\" && \"$AIDA_REGEN_PREFIX/bin/real-tool\""]);
        cfg.install_command = Some(vec![
            "/bin/sh".into(),
            "-c".into(),
            r#"
printf 'sentinel\n' > value
printf '#!/bin/sh\necho shadowed\n' > "$PREFIX/bin/grep"
mkdir -p lib
printf '#!/bin/sh\nprintf "ok   - locator tool\\n"\n' > lib/real-tool
printf '#!/bin/sh\nexec "%s/lib/real-tool"\n' "$AIDA_REGEN_TRIAL_ROOT" > "$PREFIX/bin/real-tool"
chmod +x "$PREFIX/bin/grep" "$PREFIX/bin/real-tool" lib/real-tool
/usr/bin/python3 - <<'PYTHON'
import os,sys
p=os.path.join(os.environ['HOME'], '.local/lib/python%d.%d/site-packages'%sys.version_info[:2])
os.makedirs(p)
open(os.path.join(p,'usercustomize.py'),'w').write("print('ok   - INJECTED HOME')\n")
PYTHON
"#
            .into(),
        ]);
        let record = run_trial(&cfg, dir.path(), manifest(), "m", "v", "h", false).unwrap();
        assert_eq!(record.status, "scored", "{:?}", record.error);
        assert_eq!(
            record
                .checks
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["fresh home", "locator tool"]
        );
        assert!(record.clean, "{:?}", record.flags);
    }

    #[test]
    fn confined_installer_survivor_is_recorded_without_suppressing_raw_score() {
        if !matches!(
            crate::session::bwrap_availability(),
            crate::session::BwrapAvailability::Ok
        ) {
            return;
        }
        let dir = TempDir::new().unwrap();
        let mut cfg = config(&[
            "/bin/sh",
            "-c",
            "printf 'ok   - retained raw result\\n'; exit 7",
        ]);
        cfg.install_command = Some(vec![
            "/usr/bin/python3".into(),
            "-c".into(),
            r#"
import os,time
r,w=os.pipe()
pid=os.fork()
if pid:
 os.close(w); os.read(r,1); os._exit(0)
os.setsid(); os.close(r); os.close(1); os.close(2)
os.write(w,b'x'); os.close(w)
time.sleep(5)
"#
            .into(),
        ]);
        let record = run_trial(&cfg, dir.path(), manifest(), "m", "v", "h", false).unwrap();
        assert_eq!(record.status, "scored", "{:?}", record.error);
        assert!(record.survivors_detected >= 1);
        assert!(!record.clean);
        assert!(record.flags.iter().any(|f| f == "installer_survivors"));
        assert_eq!(record.oracle_exit, Some(7));
        assert_eq!(record.aggregate.unwrap().passed, 1);
    }

    #[test]
    fn frozen_guards_cannot_be_changed_and_pipe_hits_cannot_be_truncated() {
        if !matches!(
            crate::session::bwrap_availability(),
            crate::session::BwrapAvailability::Ok
        ) {
            return;
        }
        let trial = TempDir::new().unwrap();
        let mut cfg = config(&["/bin/true"]);
        cfg.required_tools = vec!["sh".into()];
        let mut env = ScoreEnv::new(&cfg, false).unwrap();
        let guard = env.guards.root.join("bin/sh");
        let output = env.run(trial.path(), &["/bin/sh".into(), "-c".into(), "sh -c true; : > \"$HOME/guard-hits\"; : > \"$AIDA_REGEN_GUARD_PIPE\"; if printf bad > \"$1\"; then exit 1; fi".into(), "probe".into(), guard.to_str().unwrap().into()]).unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let protection = env
            .run(
                trial.path(),
                &[
                    "/usr/bin/python3".into(),
                    "-c".into(),
                    r#"
import os,errno
p=os.environ['AIDA_REGEN_GUARD_PIPE']
def refused(f, expected):
 try: f()
 except OSError as e: assert e.errno == expected, e; return
 raise AssertionError('guard evidence was mutable/readable')
refused(lambda: os.open(p,os.O_RDONLY|os.O_NONBLOCK),errno.EACCES)
refused(lambda: os.chmod(p,0o600),errno.EROFS)
refused(lambda: os.unlink(p),errno.EROFS)
f=os.open(p,os.O_WRONLY|os.O_NONBLOCK)
refused(lambda: os.fchmod(f,0o600),errno.EROFS)
refused(lambda: os.ftruncate(f,0),errno.EINVAL)
os.close(f)
"#
                    .into(),
                ],
            )
            .unwrap();
        assert!(
            protection.status.success(),
            "{}",
            String::from_utf8_lossy(&protection.stderr)
        );
        assert_eq!(env.guard_hits().unwrap(), ["sh"]);
        // Host-side corruption is also refused, before the next child starts.
        std::fs::write(guard, "tampered").unwrap();
        assert!(env.run(trial.path(), &["/bin/true".into()]).is_err());
    }

    #[test]
    fn bare_required_system_tool_uses_guard_and_forbids_clean() {
        if !matches!(
            crate::session::bwrap_availability(),
            crate::session::BwrapAvailability::Ok
        ) {
            return;
        }
        let dir = TempDir::new().unwrap();
        let mut cfg = config(&[
            "/bin/sh",
            "-c",
            "grep sentinel \"$AIDA_REGEN_TRIAL_ROOT/value\" && printf 'ok   - host fallback\\n'",
        ]);
        cfg.required_tools = vec!["grep".into()];
        std::fs::write(dir.path().join("value"), "sentinel\n").unwrap();
        let record = run_trial(&cfg, dir.path(), manifest(), "m", "v", "h", false).unwrap();
        assert_eq!(record.status, "scored", "{:?}", record.error);
        assert_eq!(record.guard_hits, ["grep"]);
        assert!(!record.clean);
    }

    #[test]
    fn python_default_subprocess_records_guard_hit_without_inherited_fd() {
        if !matches!(
            crate::session::bwrap_availability(),
            crate::session::BwrapAvailability::Ok
        ) {
            return;
        }
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("value"), "sentinel\n").unwrap();
        let mut cfg = config(&[
            "/usr/bin/python3",
            "-c",
            r#"
import os,subprocess
r=subprocess.run(['grep','sentinel',os.environ['AIDA_REGEN_TRIAL_ROOT']+'/value'],capture_output=True,text=True)
print(('ok   - ' if r.returncode == 0 and r.stdout == 'sentinel\n' else 'FAIL - ')+'subprocess tool')
"#,
        ]);
        cfg.required_tools = vec!["grep".into()];
        let record = run_trial(&cfg, dir.path(), manifest(), "m", "v", "h", false).unwrap();
        assert_eq!(record.status, "scored", "{:?}", record.error);
        assert_eq!(record.aggregate.unwrap().passed, 1);
        assert_eq!(record.guard_hits, ["grep"]);
        assert!(!record.clean);
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
        cfg.install_command = Some(vec!["/bin/true".into()]);
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
        let mut cfg = config(&["/bin/sh", "-c", "echo 'ok   - never'"]);
        cfg.install_command = None;
        let record = run_trial(&cfg, dir.path(), manifest(), "m", "v", "h", true).unwrap();
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

    // trace:STORY-1676 | ai:codex
    #[test]
    fn own_installer_keeps_bin_and_lib_layouts_equivalent_and_missing_install_is_distinct() {
        let mut cfg = config(&["/bin/sh", "-c", "test \"$(\"$AIDA_REGEN_PREFIX/bin/tool\")\" = real && printf 'ok   - installed tool\\n'"]);
        cfg.install_command = None;
        for layout in ["bin", "lib"] {
            let source = TempDir::new().unwrap();
            std::fs::create_dir(source.path().join(layout)).unwrap();
            std::fs::write(
                source.path().join(layout).join("tool"),
                "#!/bin/sh\nprintf real\n",
            )
            .unwrap();
            std::fs::set_permissions(
                source.path().join(layout).join("tool"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            let installer = format!(
                r#"#!/bin/sh
set -eu
test -z "${{AIDA_REGEN_CHECK_RESULTS+x}}"
mkdir -p "$PREFIX/{layout}" "$PREFIX/bin"
cp {layout}/tool "$PREFIX/{layout}/tool"
"#
            );
            let installer = if layout == "lib" {
                format!("{installer}printf '#!/bin/sh\\nexec \"%s/lib/tool\"\\n' \"$PREFIX\" > \"$PREFIX/bin/tool\"\nchmod +x \"$PREFIX/bin/tool\"\n")
            } else {
                installer
            };
            std::fs::write(source.path().join("install.sh"), installer).unwrap();
            std::fs::set_permissions(
                source.path().join("install.sh"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            let result = run_trial(&cfg, source.path(), manifest(), "m", "v", "h", true).unwrap();
            assert_eq!(result.status, "scored", "{:?}", result.error);
            assert_eq!(result.install_exit, Some(0));
            assert_eq!(result.aggregate.unwrap().pass_rate, 1.0);
            assert_eq!(
                std::fs::read_dir(source.path()).unwrap().count(),
                2,
                "input remains untouched"
            );
        }
        let empty = TempDir::new().unwrap();
        let result = run_trial(&cfg, empty.path(), manifest(), "m", "v", "h", true).unwrap();
        assert_eq!(result.status, "install_failed");
        assert!(
            result.install_exit.is_none()
                && result.oracle_exit.is_none()
                && result.aggregate.is_none()
        );
        assert!(result.error.unwrap().contains("no declared installer"));
        cfg.install_command = Some(vec!["./missing-installer".into()]);
        let result = run_trial(&cfg, empty.path(), manifest(), "m", "v", "h", true).unwrap();
        assert_eq!(result.status, "install_failed");
        assert!(result.error.unwrap().contains("installer launch failed"));
    }

    // trace:STORY-1676 | ai:codex
    #[test]
    fn oracle_sidecar_capture_flags_and_roundtrips_without_marker_guessing() {
        let dir = TempDir::new().unwrap();
        for (diagnostic, crash, unknown) in [
            (
                r#"printf '%s\n' '{"v":1,"check":"Traceback usage 127","kind":"crash","exit":127}' > "$AIDA_REGEN_CHECK_RESULTS""#,
                1,
                0,
            ),
            (
                r#"printf '%s\n' '{"v":1,"check":"Traceback usage 127","kind":"assertion","exit":127}' > "$AIDA_REGEN_CHECK_RESULTS""#,
                0,
                0,
            ),
            (
                r#"printf '%s\n' '{"v":2,"check":"Traceback usage 127","kind":"crash"}' > "$AIDA_REGEN_CHECK_RESULTS""#,
                0,
                1,
            ),
            (":", 0, 1),
        ] {
            let script = format!("{diagnostic}\nprintf 'FAIL - Traceback usage 127\\n'");
            let mut cfg = config(&["/bin/sh", "-c", &script]);
            cfg.install_command = Some(vec![
                "/bin/sh".into(),
                "-c".into(),
                "test -z \"${AIDA_REGEN_CHECK_RESULTS+x}\"".into(),
            ]);
            let mut row = run_trial(&cfg, dir.path(), manifest(), "m", "v", "h", true).unwrap();
            assert_eq!(row.status, "scored", "{:?}", row.error);
            let counts = diagnostics::summary(&row);
            assert_eq!((counts.crashes, counts.unknown), (crash, unknown));
            diagnostics::flag_trial(&mut row);
            assert_eq!(
                row.flags.iter().any(|f| f.starts_with("crash-dominated:")),
                crash > 0
            );
            assert_eq!(
                row.flags.iter().any(|f| f.starts_with("unclassified:")),
                unknown > 0
            );
            let copy: TrialRecord =
                serde_json::from_slice(&serde_json::to_vec(&row).unwrap()).unwrap();
            assert_eq!(diagnostics::summary(&copy).unknown, unknown);
            assert_eq!(diagnostics::summary(&copy).crashes, crash);
        }
        for threshold in ["-1", "101", "256", "0.5"] {
            assert!(RegenConfig::parse(&format!("[regen]\noracle_command=['oracle.sh']\noracle_paths=['oracle.sh']\ncrash_failure_share_percent={threshold}")).is_err());
        }
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
