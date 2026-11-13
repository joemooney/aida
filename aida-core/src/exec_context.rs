//! Immutable execution inputs, shared by production entry points and fixtures.
//!
//! Capture once at an explicit request boundary and pass `&ExecContext` down.
//! A snapshot is input, never evidence of a live grant, lease or trusted path.
//! No environment values are exposed through Debug or serialization.
// trace:STORY-2538 | ai:codex

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// Lookup errors carry neither names nor values, including non-Unicode secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvLookupError {
    Missing,
    NonUnicode,
}

impl std::fmt::Display for EnvLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Missing => "missing execution environment input",
            Self::NonUnicode => "non-Unicode execution environment input",
        })
    }
}
impl std::error::Error for EnvLookupError {}

/// Parent-readable facts. Child forwarding belongs exclusively to launch_policy.
#[derive(Clone, Default)]
pub struct Environment(BTreeMap<OsString, OsString>);

impl Environment {
    pub fn from_pairs(pairs: impl IntoIterator<Item = (OsString, OsString)>) -> Self {
        Self(pairs.into_iter().collect())
    }

    /// The single ambient environment capture boundary. Does not infer HOME,
    /// authority roots, tools or capabilities from the captured values.
    pub fn capture() -> Self {
        Self::from_pairs(std::env::vars_os())
    }

    pub fn get(&self, key: impl AsRef<OsStr>) -> Option<&OsStr> {
        self.0.get(key.as_ref()).map(OsString::as_os_str)
    }

    /// Return a snapshot with an explicitly supplied boundary decision.
    pub fn with_value(mut self, key: impl Into<OsString>, value: Option<OsString>) -> Self {
        let key = key.into();
        if let Some(value) = value {
            self.0.insert(key, value);
        } else {
            self.0.remove(&key);
        }
        self
    }

    /// Missing and non-Unicode inputs remain distinguishable.
    pub fn var(&self, key: impl AsRef<OsStr>) -> Result<&str, EnvLookupError> {
        match self.get(key) {
            None => Err(EnvLookupError::Missing),
            Some(value) => value.to_str().ok_or(EnvLookupError::NonUnicode),
        }
    }
}

/// Explicit selections, not executable admission or proof of installation.
/// Keep the captured search path separate from selected tool paths.
#[derive(Clone, Default)]
pub struct ToolPaths {
    search_path: Option<OsString>,
    selected: BTreeMap<OsString, PathBuf>,
}

impl ToolPaths {
    pub fn new(
        search_path: Option<OsString>,
        selected: impl IntoIterator<Item = (OsString, PathBuf)>,
    ) -> Self {
        Self {
            search_path,
            selected: selected.into_iter().collect(),
        }
    }

    pub fn search_path(&self) -> Option<&OsStr> {
        self.search_path.as_deref()
    }

    pub fn selected(&self, name: impl AsRef<OsStr>) -> Option<&Path> {
        self.selected.get(name.as_ref()).map(PathBuf::as_path)
    }
}

/// Wall timestamps and elapsed budgets are deliberately separate. Implementors
/// must return nondecreasing monotonic durations within one clock instance.
pub trait Clock: Send + Sync {
    fn wall_now(&self) -> SystemTime;
    fn monotonic_now(&self) -> Duration;
}

pub struct SystemClock {
    origin: Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn wall_now(&self) -> SystemTime {
        SystemTime::now()
    }
    fn monotonic_now(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// No implicit cwd or operator-home fallback. Authority validation stays live
/// at the existing locked boundary; `authority_root` is merely its input.
#[derive(Clone)]
pub struct ExecContext {
    environment: Environment,
    home: Option<PathBuf>,
    project_root: PathBuf,
    authority_root: PathBuf,
    tools: ToolPaths,
    clock: Arc<dyn Clock>,
}

impl ExecContext {
    pub fn new(
        environment: Environment,
        home: Option<PathBuf>,
        project_root: PathBuf,
        authority_root: PathBuf,
        tools: ToolPaths,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            environment,
            home,
            project_root,
            authority_root,
            tools,
            clock,
        }
    }

    pub fn environment(&self) -> &Environment {
        &self.environment
    }
    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }
    pub fn authority_root(&self) -> &Path {
        &self.authority_root
    }
    pub fn tools(&self) -> &ToolPaths {
        &self.tools
    }
    pub fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct FakeClock(AtomicU64);
    impl Clock for FakeClock {
        fn wall_now(&self) -> SystemTime {
            SystemTime::UNIX_EPOCH + self.monotonic_now()
        }
        fn monotonic_now(&self) -> Duration {
            Duration::from_secs(self.0.load(Ordering::SeqCst))
        }
    }

    fn fixture(label: &str, clock: Arc<dyn Clock>) -> ExecContext {
        ExecContext::new(
            Environment::from_pairs([("PATH".into(), label.into())]),
            Some(PathBuf::from(label)),
            PathBuf::from("/scratch"),
            PathBuf::from("/authority"),
            ToolPaths::new(Some(label.into()), [("git".into(), PathBuf::from(label))]),
            clock,
        )
    }

    #[test]
    fn exec_context_fixtures_coexist_without_process_mutation() {
        let workers: Vec<_> = ["/fixture-a", "/fixture-b"]
            .into_iter()
            .map(|label| {
                std::thread::spawn(move || {
                    let ctx = fixture(label, Arc::new(FakeClock(AtomicU64::new(0))));
                    for _ in 0..100 {
                        assert_eq!(ctx.environment().var("PATH"), Ok(label));
                        assert_eq!(ctx.home(), Some(Path::new(label)));
                        assert_eq!(ctx.tools().selected("git"), Some(Path::new(label)));
                        assert_eq!(ctx.tools().search_path(), Some(OsStr::new(label)));
                        assert_ne!(ctx.project_root(), ctx.authority_root());
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn exec_context_capture_reads_only_the_subprocess_inputs() {
        // Do not capture the parent test process: legacy tests still mutate its
        // environment. Only the exact one-test subprocess crosses capture().
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "exec_context::tests::exec_context_capture_child_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env_clear()
            .env("PATH", "/explicit-fixture-path")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "child failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
    }

    #[test]
    #[ignore = "fixture executed by the parent capture test in an env-cleared subprocess"]
    fn exec_context_capture_child_fixture() {
        let environment = Environment::capture();
        assert_eq!(environment.var("PATH"), Ok("/explicit-fixture-path"));
        assert_eq!(environment.get("HOME"), None);
    }

    #[test]
    fn exec_context_clock_advances_after_capture() {
        let clock = Arc::new(FakeClock(AtomicU64::new(10)));
        let ctx = fixture("/fixture", clock.clone());
        let start = ctx.clock().monotonic_now();
        clock.0.store(25, Ordering::SeqCst);
        assert_eq!(ctx.clock().monotonic_now() - start, Duration::from_secs(15));
        assert_eq!(
            ctx.clock().wall_now(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(25)
        );
    }

    #[test]
    fn exec_context_missing_inputs_have_no_fallback() {
        let ctx = ExecContext::new(
            Environment::default(),
            None,
            "/p".into(),
            "/a".into(),
            ToolPaths::default(),
            Arc::new(SystemClock::default()),
        );
        assert!(ctx.home().is_none());
        assert!(ctx.tools().search_path().is_none());
        assert!(ctx.tools().selected("git").is_none());
        assert_eq!(ctx.environment().get("HOME"), None);
        assert_eq!(
            ctx.environment().var("MISSING"),
            Err(EnvLookupError::Missing)
        );
    }

    #[cfg(unix)]
    #[test]
    fn exec_context_preserves_non_unicode() {
        use std::os::unix::ffi::OsStringExt;
        let value = OsString::from_vec(vec![0xff]);
        let key = OsString::from_vec(vec![0xfe]);
        let env = Environment::from_pairs([(key.clone(), value.clone())]);
        assert_eq!(env.get(&key), Some(value.as_os_str()));
        assert_eq!(env.var(&key), Err(EnvLookupError::NonUnicode));
        assert_eq!(format!("{:?}", env.var(&key)), "Err(NonUnicode)");
    }
}
