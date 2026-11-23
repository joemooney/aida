// trace:TASK-3784 | ai:codex
pub(crate) use crate::agent_registry::test_support::AgentSessionIsolation;
pub(crate) use aida_testkit::test_env::*;
use std::ffi::{OsStr, OsString};
use std::path::Path;

/// A private disposable root visible through bwrap's private /tmp mount.
// trace:TASK-3133 TASK-3838 | ai:codex
pub(crate) fn bwrap_fixture_parent() -> std::path::PathBuf {
    // Lane jobs have a read-only host cache and a private bwrap /dev.
    // Elsewhere libtest redirects HOME to /tmp before the tests run.
    let env = aida_core::exec_context::Environment::capture();
    let cache = if env
        .get(crate::headless_background::LANE_MARKER_ENV)
        .is_some()
    {
        Path::new("/dev/shm").to_owned()
    } else {
        crate::test_home::operator_real_home()
            .expect("bwrap fixture scratch needs the original cache home")
            .join(".cache")
    };
    std::fs::create_dir_all(&cache)
        .unwrap_or_else(|err| panic!("create scratch cache {cache:?}: {err}"));
    let cache = cache.canonicalize().expect("resolve bwrap fixture cache");
    assert!(
        !cache.starts_with("/tmp")
            && !cache.starts_with("/var/tmp")
            && !cache.ancestors().any(|p| p.ends_with("target/worktrees")),
        "bwrap fixture cache overlaps temporary or seat target paths: {cache:?}"
    );
    if let Some(target) = env.get("CARGO_TARGET_DIR") {
        let target = std::fs::canonicalize(target)
            .unwrap_or_else(|err| panic!("resolve CARGO_TARGET_DIR {target:?}: {err}"));
        assert!(
            !cache.starts_with(&target),
            "bwrap fixture cache {cache:?} inside CARGO_TARGET_DIR {target:?}"
        );
    }
    cache
}

pub(crate) fn bwrap_fixture_scratch() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let cache = bwrap_fixture_parent();
    tempfile::Builder::new()
        .prefix("aida-test-scratch-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(&cache)
        .unwrap_or_else(|err| panic!("allocate bwrap fixture scratch in {cache:?}: {err}"))
}

pub(crate) struct EnvVarsGuard(aida_testkit::test_env::EnvVarsGuard);
impl EnvVarsGuard {
    pub(crate) fn apply(pairs: &[(&'static str, Option<&str>)]) -> Self {
        let inner = aida_testkit::test_env::EnvVarsGuard::apply(pairs);
        crate::session::test_support::forget_contained_export(pairs.iter().map(|(key, _)| *key));
        Self(inner.with_cleanup(cleanup))
    }
    pub(crate) fn set(pairs: &[(&'static str, &str)]) -> Self {
        Self::apply(
            &pairs
                .iter()
                .map(|(key, value)| (*key, Some(*value)))
                .collect::<Vec<_>>(),
        )
    }
    pub(crate) fn snapshot(keys: &[&'static str]) -> Self {
        Self(aida_testkit::test_env::EnvVarsGuard::snapshot(keys).with_cleanup(cleanup))
    }
    pub(crate) fn set_key(&mut self, key: &'static str, value: impl AsRef<OsStr>) {
        self.0.set_key(key, value);
    }
    pub(crate) fn unset_key(&mut self, key: &'static str) {
        self.0.unset_key(key);
    }
}
fn cleanup(keys: &[(&'static str, Option<OsString>)]) {
    crate::session::test_support::forget_contained_export(keys.iter().map(|(key, _)| *key));
}
#[cfg(test)]
mod tests {
    use super::*;
    /// TASK-1532 acceptance 2: A source-scan test fails on any raw
    /// set_var/remove_var in test code outside the helpers.
    // trace:TASK-1532 | ai:agy
    #[test]
    fn no_raw_env_writes_in_test_code() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut offenders = scan_raw_env_writes_in_dir(&manifest_dir.join("src"));
        if manifest_dir.join("tests").is_dir() {
            offenders.extend(scan_raw_env_writes_in_dir(&manifest_dir.join("tests")));
        }
        // trace:TASK-3784 | ai:codex
        let testkit = manifest_dir.join("../aida-testkit");
        for directory in [testkit.join("src"), testkit.join("tests")] {
            if directory.is_dir() {
                offenders.extend(scan_raw_env_writes_in_dir(&directory));
            }
        }
        assert!(
            offenders.is_empty(),
            "found raw env::set_var / remove_var in test code outside helpers:\n{}",
            offenders.join("\n")
        );
    }

    /// General test-isolation hygiene discovered during BUG-2118:
    /// `std::env::set_current_dir` mutates the process-wide cwd across all
    /// libtest threads; any test that calls `set_current_dir(` directly must
    /// isolate in a one-test child via `run_path_isolated_test()`.
    ///
    /// Scope limitation: this source scanner checks literal `set_current_dir(`
    /// calls inside test files (`src/tests/**`, `tests/**`) and `#[cfg(...test...)]`
    /// blocks. Indirect cwd mutation through production code
    /// (`queue_cmd/work.rs::handle_queue_work`,
    /// `session_start.rs:2516`, `agent_launch.rs`, `reconstitute.rs`,
    /// `lib_part1.rs`, `session_end.rs`, ...) is not detected; tests reaching
    /// those production paths in-process must opt into `run_path_isolated_test()`
    /// explicitly.
    // trace:TASK-1970 | ai:antigravity
    #[test]
    fn no_unisolated_set_current_dir_in_test_code() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut offenders = scan_unisolated_set_current_dir_in_dir(&manifest_dir.join("src"));
        if manifest_dir.join("tests").is_dir() {
            offenders.extend(scan_unisolated_set_current_dir_in_dir(
                &manifest_dir.join("tests"),
            ));
        }
        // trace:TASK-3784 | ai:codex
        let testkit = manifest_dir.join("../aida-testkit");
        for directory in [testkit.join("src"), testkit.join("tests")] {
            if directory.is_dir() {
                offenders.extend(scan_unisolated_set_current_dir_in_dir(&directory));
            }
        }
        assert!(
            offenders.is_empty(),
            "found set_current_dir in test code without run_path_isolated_test():\n{}",
            offenders.join("\n")
        );
    }
}
// trace:BUG-1994 | ai:codex
#[cfg(test)]
#[path = "tests/bug_1994_path_isolation_tests.rs"]
mod bug_1994_path_isolation_tests;

#[cfg(test)]
mod documentation_tests {
    // Check the attributes Rust actually attaches to each item, so the const
    // cannot silently take the isolation helper's rustdoc again.
    // trace:TASK-2189 | ai:codex
    #[test]
    fn path_isolation_rustdoc_belongs_to_the_function() {
        let source = syn::parse_file(include_str!("../../aida-testkit/src/test_env.rs")).unwrap();
        let docs = |attrs: &[syn::Attribute]| {
            attrs
                .iter()
                .filter_map(|attr| {
                    if !attr.path().is_ident("doc") {
                        return None;
                    }
                    let syn::Meta::NameValue(meta) = &attr.meta else {
                        return None;
                    };
                    let syn::Expr::Lit(expr) = &meta.value else {
                        return None;
                    };
                    let syn::Lit::Str(value) = &expr.lit else {
                        return None;
                    };
                    Some(value.value())
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let function = source
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Fn(item) if item.sig.ident == "run_path_isolated_test" => {
                    Some(docs(&item.attrs))
                }
                _ => None,
            })
            .unwrap();
        let constant = source
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Const(item) if item.ident == "REQUIRE_BWRAP_LIVE_ENV" => {
                    Some(docs(&item.attrs))
                }
                _ => None,
            })
            .unwrap();
        assert!(
            function.contains("Returns true in the parent"),
            "{function}"
        );
        assert!(
            !constant.contains("Returns true in the parent"),
            "{constant}"
        );
        assert!(constant.contains("live-bwrap CI gate"), "{constant}");
    }
}
