//! Lane launches refused below the disk floor, retried by the scheduled disk
//! guard once the floor holds again.
//!
//! A standalone launch refused by the lane disk preflight rolls back its
//! setup (worktree, lease, claim) and records its own command line here. When
//! a scheduled guard run ends with every floor finding cleared, it re-runs
//! each recorded command as a fresh `aida` process. That process repeats every
//! ordinary launch check: authority, queue state, lease, and the disk
//! preflight itself, which records the launch again if the floor is still
//! breached. Nothing about authority is recorded or replayed.
// trace:BUG-2684 | ai:claude

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Records live in the main checkout's `.aida/`, outside any lane sandbox.
const DIR: &str = "lane-launch-retry";
/// A launch refused this long ago is dropped rather than retried.
const MAX_AGE_HOURS: i64 = 24;
/// Carries a retried launch's first refusal time into its own re-record.
pub(crate) const RETRY_SINCE_ENV: &str = "AIDA_LANE_RETRY_SINCE";

/// Marks a lane disk preflight refusal (as opposed to a failed measurement).
#[derive(Debug)]
pub(crate) struct LaneSpaceRefused(pub(crate) String);

impl std::fmt::Display for LaneSpaceRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LaneSpaceRefused {}

#[derive(Debug, Serialize, Deserialize)]
struct Refused {
    spec: String,
    /// Arguments after the program name; the program is always `aida`.
    args: Vec<String>,
    cwd: PathBuf,
    first_refused_at: chrono::DateTime<chrono::Utc>,
    reason: String,
}

fn dir(main: &Path) -> PathBuf {
    main.join(".aida").join(DIR)
}

fn valid_spec(spec: &str) -> bool {
    !spec.is_empty()
        && spec.len() <= 64
        && spec
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        && !spec.starts_with('.')
}

/// Record a refused launch of `spec` for retry when `error` is a disk-floor
/// refusal, and return the error to report (saying it will be retried).
pub(crate) fn defer(main: &Path, spec: &str, error: anyhow::Error) -> anyhow::Error {
    if error.downcast_ref::<LaneSpaceRefused>().is_none() {
        return error;
    }
    let args: Option<Vec<String>> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string().ok())
        .collect();
    let recorded = args
        .context("the command line is not valid UTF-8")
        .and_then(|args| {
            let cwd = std::env::current_dir()?;
            record(main, spec, args, cwd, &format!("{error:#}"))
        });
    match recorded {
        Ok(()) => error.context(
            "the launch is recorded and will be retried automatically once the scheduled \
             disk guard restores the free-space floor",
        ),
        Err(why) => error.context(format!(
            "the launch could not be recorded for retry: {why:#}"
        )),
    }
}

/// Persist one refused launch, replacing an earlier record for the same spec.
pub(crate) fn record(
    main: &Path,
    spec: &str,
    args: Vec<String>,
    cwd: PathBuf,
    reason: &str,
) -> Result<()> {
    anyhow::ensure!(valid_spec(spec), "unrecordable spec id {spec:?}");
    let first_refused_at = std::env::var(RETRY_SINCE_ENV)
        .ok()
        .and_then(|since| chrono::DateTime::parse_from_rfc3339(&since).ok())
        .map_or_else(chrono::Utc::now, |since| since.with_timezone(&chrono::Utc));
    let row = Refused {
        spec: spec.to_owned(),
        args,
        cwd,
        first_refused_at,
        reason: reason.to_owned(),
    };
    let dir = dir(main);
    std::fs::create_dir_all(&dir)?;
    restrict(&dir, 0o700)?;
    let tmp = dir.join(format!(".{spec}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&row)?)?;
    restrict(&tmp, 0o600)?;
    std::fs::rename(&tmp, dir.join(format!("{spec}.json")))?;
    Ok(())
}

fn restrict(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("restricting {}", path.display()))
}

/// A record the guard may act on: a regular file (not a symlink) owned by
/// this user and writable by no one else.
fn trusted(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path)?;
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    anyhow::ensure!(
        meta.file_type().is_file() || meta.file_type().is_dir(),
        "{} is not a regular file or directory",
        path.display()
    );
    anyhow::ensure!(
        meta.uid() == uid && meta.mode() & 0o022 == 0,
        "{} is not private to this user",
        path.display()
    );
    Ok(())
}

/// Retry every recorded launch. Called by the scheduled guard only after its
/// final probe found no floor breach.
pub(crate) fn retry_refused(main: &Path) {
    retry_refused_with(main, &crate::aida_exe_path(), chrono::Utc::now());
}

fn retry_refused_with(main: &Path, exe: &Path, now: chrono::DateTime<chrono::Utc>) {
    let dir = dir(main);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            eprintln!("warning: refused lane launches not retried: {error}");
            return;
        }
    };
    if let Err(error) = trusted(&dir) {
        eprintln!("warning: refused lane launches not retried: {error:#}");
        return;
    }
    let mut records: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "json")
                && path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(valid_spec)
        })
        .collect();
    records.sort();
    for path in records {
        if let Err(error) = retry_one(&path, exe, now) {
            eprintln!(
                "warning: refused lane launch {} not retried: {error:#}",
                path.display()
            );
        }
    }
}

fn retry_one(path: &Path, exe: &Path, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
    trusted(path)?;
    // Claim the record so a concurrent guard run cannot launch it twice.
    let claimed = path.with_extension("retrying");
    std::fs::rename(path, &claimed)?;
    let outcome = launch(&claimed, exe, now);
    let _ = std::fs::remove_file(&claimed);
    outcome
}

fn launch(claimed: &Path, exe: &Path, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let row: Refused = serde_json::from_slice(&std::fs::read(claimed)?)?;
    let stem = claimed.file_stem().and_then(|stem| stem.to_str());
    anyhow::ensure!(stem == Some(row.spec.as_str()), "record names another spec");
    if now - row.first_refused_at > chrono::Duration::hours(MAX_AGE_HOURS) {
        eprintln!(
            "refused lane launch of {} dropped: first refused more than {MAX_AGE_HOURS} hours ago",
            row.spec
        );
        return Ok(());
    }
    let log = claimed.with_extension("log");
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(&row.args)
        .current_dir(&row.cwd)
        .stdin(std::process::Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out)
        // The retried launch is an ordinary launch, not a scheduled child,
        // and must outlive the guard's process group.
        .env_remove("AIDA_SCHEDULE_CHILD")
        .env(RETRY_SINCE_ENV, row.first_refused_at.to_rfc3339())
        .process_group(0);
    crate::standalone_implementer::scrub_standalone_child_env(&mut cmd);
    let child = cmd
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))?;
    eprintln!(
        "retried the lane launch of {} refused below the disk floor (pid {}, log {})",
        row.spec,
        child.id(),
        log.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(path: &Path) -> String {
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(path) {
                if !text.is_empty() {
                    return text;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("{} never appeared", path.display());
    }

    #[test]
    fn bug_2684_only_disk_floor_refusals_are_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let other = defer(tmp.path(), "BUG-1", anyhow::anyhow!("cannot measure"));
        assert_eq!(other.to_string(), "cannot measure");
        assert!(!dir(tmp.path()).exists());
        let refused = defer(
            tmp.path(),
            "BUG-1",
            anyhow::Error::new(LaneSpaceRefused("lane launch refused".into())),
        );
        assert!(format!("{refused:#}").contains("retried automatically"));
        assert!(dir(tmp.path()).join("BUG-1.json").is_file());
        assert!(record(tmp.path(), "../x", vec![], tmp.path().into(), "r").is_err());
    }

    #[test]
    fn bug_2684_retry_skips_untrusted_and_stale_records() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let marker = root.join("ran");
        let args = vec!["-c".into(), format!("echo ran >> {}", marker.display())];
        record(root, "BUG-1", args.clone(), root.into(), "refused").unwrap();
        let path = dir(root).join("BUG-1.json");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        retry_refused_with(root, Path::new("/bin/sh"), chrono::Utc::now());
        assert!(path.exists(), "a record others can write is not run");

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let later = chrono::Utc::now() + chrono::Duration::hours(MAX_AGE_HOURS + 1);
        retry_refused_with(root, Path::new("/bin/sh"), later);
        assert!(!path.exists(), "a stale record is dropped");
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!marker.exists());

        record(root, "BUG-1", args, root.into(), "refused").unwrap();
        retry_refused_with(root, Path::new("/bin/sh"), chrono::Utc::now());
        assert_eq!(wait_for(&marker), "ran\n");
        assert!(!path.exists());
    }
}
