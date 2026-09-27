//! The crate's home- and config-directory resolvers.
//!
//! On unix `dirs::home_dir()` and `dirs::config_dir()` already derive from
//! `$HOME` (directly, or via `$XDG_CONFIG_HOME` / `Library/Application
//! Support`), so redirecting the environment at a temp home redirects them
//! too. On Windows both resolve through `SHGetKnownFolderPath` and ignore
//! `HOME`, `USERPROFILE` and `APPDATA` entirely — so a redirected
//! environment (the lib-test fence, a CI runner, a scripted sandbox) still
//! lands in the operator's real profile there.
//!
//! Every home/config lookup in this crate goes through [`home_dir`] and
//! [`config_dir`], which consult the environment first and fall back to the
//! platform lookup. The answer is unchanged on unix and macOS; on Windows it
//! becomes overridable. A source-scan test in this module keeps new code from
//! calling the `dirs` crate, or reading the vars, directly.
// trace:TASK-1513 | ai:claude

use std::path::PathBuf;

/// A path from a raw env value, treating an empty value as unset — which is
/// what `dirs` does with an empty `$HOME`.
fn non_empty(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    value.filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// `$key` as a path when it is set and non-empty.
fn non_empty_env(key: &str) -> Option<PathBuf> {
    non_empty(std::env::var_os(key))
}

/// Home from the environment alone, in precedence order. Split out from
/// [`home_dir`] so the order is testable without mutating the process
/// environment.
// trace:TASK-1513 | ai:claude
fn home_from_env(get: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    get("HOME").or_else(|| get("USERPROFILE"))
}

/// Config directory from the environment alone. Windows only: elsewhere the
/// platform lookup is already `$HOME`-derived, and overriding it here would
/// change where `aida` reads its config on unix.
// trace:TASK-1513 | ai:claude
#[cfg(windows)]
fn config_from_env(get: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    get("APPDATA").or_else(|| home_from_env(get).map(|h| h.join("AppData").join("Roaming")))
}

fn platform_home_dir() -> Option<PathBuf> {
    #[cfg(feature = "native")]
    {
        dirs::home_dir() // allow-direct-home-dir
    }
    #[cfg(not(feature = "native"))]
    {
        None
    }
}

fn platform_config_dir() -> Option<PathBuf> {
    #[cfg(feature = "native")]
    {
        dirs::config_dir() // allow-direct-home-dir
    }
    #[cfg(not(feature = "native"))]
    {
        None
    }
}

/// The user's home directory: `$HOME`, then `$USERPROFILE`, then the
/// platform lookup. `None` when none of them answers.
// trace:TASK-1513 | ai:claude
pub fn home_dir() -> Option<PathBuf> {
    home_from_env(&non_empty_env).or_else(platform_home_dir)
}

/// The user's per-user config directory (`~/.config` on unix,
/// `~/Library/Application Support` on macOS, the roaming app-data dir on
/// Windows). `None` when the home directory is unknown.
// trace:TASK-1513 | ai:claude
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(dir) = config_from_env(&non_empty_env) {
            return Some(dir);
        }
    }
    platform_config_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A stand-in for the process environment, so the precedence tests never
    /// read or write the real one.
    fn env_of(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<PathBuf> {
        let owned: Vec<(&'static str, PathBuf)> =
            pairs.iter().map(|(k, v)| (*k, PathBuf::from(*v))).collect();
        move |key: &str| {
            owned
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
        }
    }

    /// `$HOME` wins; `$USERPROFILE` is the Windows-side fallback that makes
    /// the redirect effective where `dirs` ignores the environment.
    // trace:TASK-1513 | ai:claude
    #[test]
    fn home_from_env_prefers_home_then_userprofile() {
        let both = env_of(&[("HOME", "/fixture/home"), ("USERPROFILE", "/fixture/up")]);
        assert_eq!(home_from_env(&both), Some(PathBuf::from("/fixture/home")));

        let profile_only = env_of(&[("USERPROFILE", "/fixture/up")]);
        assert_eq!(
            home_from_env(&profile_only),
            Some(PathBuf::from("/fixture/up"))
        );

        assert_eq!(home_from_env(&env_of(&[])), None);
    }

    /// On Windows the roaming app-data dir is taken from `$APPDATA`, and a
    /// redirected home still wins when it is unset.
    // trace:TASK-1513 | ai:claude
    #[cfg(windows)]
    #[test]
    fn config_from_env_prefers_appdata_then_the_redirected_home() {
        let with_appdata = env_of(&[
            ("APPDATA", r"C:\fixture\Roaming"),
            ("USERPROFILE", r"C:\fixture\profile"),
        ]);
        assert_eq!(
            config_from_env(&with_appdata),
            Some(PathBuf::from(r"C:\fixture\Roaming"))
        );

        let home_only = env_of(&[("HOME", r"C:\fixture\home")]);
        assert_eq!(
            config_from_env(&home_only),
            Some(
                PathBuf::from(r"C:\fixture\home")
                    .join("AppData")
                    .join("Roaming")
            )
        );

        assert_eq!(config_from_env(&env_of(&[])), None);
    }

    /// An empty `$HOME` is not a home: it must fall through to the next
    /// candidate rather than resolve to the current directory.
    // trace:TASK-1513 | ai:claude
    #[test]
    fn an_empty_value_counts_as_unset() {
        assert_eq!(non_empty(Some(std::ffi::OsString::from(""))), None);
        assert_eq!(non_empty(None), None);
        assert_eq!(
            non_empty(Some(std::ffi::OsString::from("/fixture/home"))),
            Some(PathBuf::from("/fixture/home"))
        );
    }

    /// No code in this crate may resolve a home or config directory itself.
    /// `dirs::home_dir()` / `dirs::config_dir()` ignore the environment on
    /// Windows, and a direct `HOME` / `USERPROFILE` / `APPDATA` read skips
    /// the fallback order above — both reintroduce the Windows gap this
    /// module closes. Route new code through [`home_dir`] / [`config_dir`];
    /// a deliberate exception carries an `allow-direct-home-dir` marker on
    /// the line.
    // trace:TASK-1513 | ai:claude
    #[test]
    fn no_direct_home_resolution_outside_this_module() {
        let mut needles = vec![
            ["dirs", "::", "home_dir"].concat(),
            ["dirs", "::", "config_dir"].concat(),
        ];
        for key in ["HOME", "USERPROFILE", "APPDATA"] {
            needles.push(format!("var(\"{key}\")"));
            needles.push(format!("var_os(\"{key}\")"));
        }

        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().is_none_or(|e| e != "rs") || p.ends_with("home.rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap();
                for (i, line) in text.lines().enumerate() {
                    if line.trim_start().starts_with("//") || line.contains("allow-direct-home-dir")
                    {
                        continue;
                    }
                    if needles.iter().any(|n| line.contains(n)) {
                        offenders.push(format!("{}:{}", p.display(), i + 1));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "use crate::home::home_dir() / crate::home::config_dir() instead of resolving \
             a home directory directly:\n{}",
            offenders.join("\n")
        );
    }
}
