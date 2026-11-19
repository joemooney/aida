//! Operator-requested, read-only observations of lane launcher binaries.
// trace:TASK-3812 | ai:codex
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

#[derive(Debug, serde::Serialize)]
struct Observation {
    host: String,
    binary_sha: Option<String>,
    age_seconds: Option<i64>,
    dirty_build: Option<bool>,
    status: &'static str,
    changes_behind: Option<u64>,
    detail: Option<String>,
}

fn valid_host(host: &str) -> bool {
    host.len() <= 253
        && host
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && host
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'.' | b'-' | b'_'))
}

fn read(cmd: Command) -> Result<String> {
    let (status, bytes) = aida_core::transport_policy::bounded(
        cmd,
        &[],
        8192,
        Instant::now() + Duration::from_secs(10),
    )?;
    if !status.success() {
        bail!("probe exited with {status}");
    }
    Ok(String::from_utf8(bytes)?.trim().to_owned())
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = aida_core::git_ops::host_git_command(root);
    cmd.args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    read(cmd)
}

fn banner_fields(banner: &str, now: i64) -> Result<(&str, i64, bool)> {
    let (program, stamp) = banner
        .split_once(" (built ")
        .context("missing build stamp")?;
    anyhow::ensure!(
        program.starts_with("aida ") && !banner.contains('\n'),
        "unexpected version output"
    );
    let (time, sha) = stamp
        .strip_suffix(')')
        .and_then(|stamp| stamp.rsplit_once(", sha "))
        .context("malformed build stamp")?;
    let revision = sha.strip_suffix("+dirty").unwrap_or(sha);
    anyhow::ensure!(
        (7..=64).contains(&revision.len()) && revision.bytes().all(|ch| ch.is_ascii_hexdigit()),
        "invalid build SHA"
    );
    // The remote probe fixes TZ to UTC. GNU and chrono may render it as
    // UTC, +00:00, or +0000 depending on the binary's build.
    let (time, zone) = time.rsplit_once(' ').context("missing build time zone")?;
    anyhow::ensure!(
        matches!(zone, "UTC" | "GMT" | "+00:00" | "+0000"),
        "build time zone was not UTC"
    );
    let built = chrono::NaiveDateTime::parse_from_str(time, "%Y-%m-%d %H:%M:%S")?
        .and_utc()
        .timestamp();
    Ok((revision, now.saturating_sub(built).max(0), sha != revision))
}

fn observe(
    host: &str,
    banner: Result<String>,
    root: &Path,
    target: Option<&str>,
    now: i64,
) -> Observation {
    let mut row = Observation {
        host: host.into(),
        binary_sha: None,
        age_seconds: None,
        dirty_build: None,
        status: "unknown",
        changes_behind: None,
        detail: None,
    };
    let result = (|| -> Result<()> {
        let banner = banner?;
        let (sha, age, dirty) = banner_fields(&banner, now)?;
        row.binary_sha = Some(sha.into());
        row.age_seconds = Some(age);
        row.dirty_build = Some(dirty);
        let target = target.context("local origin/main is unavailable")?;
        let full = git(
            root,
            &["rev-parse", "--verify", &format!("{sha}^{{commit}}")],
        )
        .context("build commit cannot be resolved locally")?;
        anyhow::ensure!(
            full.starts_with(&sha.to_ascii_lowercase()),
            "build SHA resolved to a ref with a different object ID"
        );
        let counts = git(
            root,
            &[
                "rev-list",
                "--left-right",
                "--count",
                &format!("{full}...{target}"),
            ],
        )?;
        let (ahead, _) = counts.split_once('\t').context("malformed Git count")?;
        let behind = git(
            root,
            &[
                "rev-list",
                "--first-parent",
                "--count",
                &format!("{full}..{target}"),
            ],
        )?
        .parse()?;
        row.status = match (ahead.parse::<u64>()?, behind) {
            (0, 0) => "matches",
            (0, _) => "behind",
            (_, 0) => "ahead",
            (_, _) => "diverged",
        };
        row.changes_behind = Some(behind);
        Ok(())
    })();
    if let Err(err) = result {
        row.detail = Some(format!("{err:#}"));
    }
    row
}

fn version(host: &str) -> Result<String> {
    let mut cmd = Command::new("ssh");
    cmd.args([
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=5",
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "UpdateHostKeys=no",
        "-o",
        "ControlPath=none",
        "--",
        host,
        r#"export TZ=UTC PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"; exec aida --version"#,
    ]);
    read(cmd)
}

pub(crate) fn run(root: &Path, hosts: &[String], json: bool) -> Result<()> {
    anyhow::ensure!(
        hosts.iter().all(|host| valid_host(host)),
        "use SSH host aliases without user names, ports, or options"
    );
    let target = git(
        root,
        &["rev-parse", "--verify", "refs/remotes/origin/main^{commit}"],
    )
    .ok();
    let now = chrono::Utc::now().timestamp();
    let rows: Vec<_> = hosts
        .iter()
        .map(|host| observe(host, version(host), root, target.as_deref(), now))
        .collect();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "origin_main_sha": target,
                "hosts": rows,
            }))?
        );
    } else {
        println!(
            "Local origin/main: {}",
            target.as_deref().unwrap_or("unknown")
        );
        for row in rows {
            let age = row
                .age_seconds
                .map(|secs| crate::humanize_duration_secs(secs as u64))
                .unwrap_or_else(|| "unknown".into());
            let dirty = if row.dirty_build == Some(true) {
                "+dirty"
            } else {
                ""
            };
            let behind = row
                .changes_behind
                .map(|n| format!(", {n} changes behind"))
                .unwrap_or_default();
            let detail = row
                .detail
                .map(|why| format!(" ({why})"))
                .unwrap_or_default();
            println!(
                "{}: binary {}{dirty}, age {age}, {}{behind}{detail}",
                row.host,
                row.binary_sha.as_deref().unwrap_or("unknown"),
                row.status
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_git(root: &Path, args: &[&str]) -> String {
        let mut cmd = crate::test_git::git_command();
        cmd.current_dir(root)
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.org"])
            .args(args);
        read(cmd).unwrap()
    }

    #[test]
    fn lane_binaries_compares_each_report_to_the_pinned_tip() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        fixture_git(root, &["init", "-b", "main"]);
        fixture_git(root, &["commit", "--allow-empty", "-m", "first"]);
        let first = fixture_git(root, &["rev-parse", "--short", "HEAD"]);
        fixture_git(root, &["commit", "--allow-empty", "-m", "second"]);
        let tip = fixture_git(root, &["rev-parse", "HEAD"]);
        fixture_git(root, &["commit", "--allow-empty", "-m", "third"]);
        let ahead = fixture_git(root, &["rev-parse", "HEAD"]);
        fixture_git(root, &["checkout", "--detach", &first]);
        fixture_git(root, &["commit", "--allow-empty", "-m", "diverged"]);
        let diverged = fixture_git(root, &["rev-parse", "HEAD"]);
        let stamp = |sha: &str| {
            Ok(format!(
                "aida 0.16.0 (built 2024-01-01 00:00:00 +00:00, sha {sha})"
            ))
        };
        let now = 1_704_067_260;
        let old = observe(
            "old",
            stamp(&format!("{first}+dirty")),
            root,
            Some(&tip),
            now,
        );
        assert_eq!(old.binary_sha.as_deref(), Some(first.as_str()));
        assert_eq!(old.age_seconds, Some(60));
        assert_eq!(old.dirty_build, Some(true));
        assert_eq!((old.status, old.changes_behind), ("behind", Some(1)));
        assert_eq!(
            observe("current", stamp(&tip), root, Some(&tip), now).status,
            "matches"
        );
        assert_eq!(
            observe("ahead", stamp(&ahead), root, Some(&tip), now).status,
            "ahead"
        );
        assert_eq!(
            observe("diverged", stamp(&diverged), root, Some(&tip), now).status,
            "diverged"
        );
        fixture_git(root, &["tag", "deadbeef", &tip]);
        let missing = observe("unknown-ref", stamp("deadbeef"), root, Some(&tip), now);
        assert_eq!(missing.status, "unknown");
        assert_eq!(missing.binary_sha.as_deref(), Some("deadbeef"));
        assert_eq!(
            observe(
                "offline",
                Err(anyhow::anyhow!("unreachable")),
                root,
                Some(&tip),
                now
            )
            .status,
            "unknown"
        );
        assert_eq!(
            observe("no-main", stamp(&tip), root, None, now).status,
            "unknown"
        );
    }

    #[test]
    fn lane_binaries_rejects_option_like_aliases_and_bad_stamps() {
        assert!(valid_host("dell1-cf"));
        for host in ["", "-oProxyCommand", "user@host", "host:22", "dell1\n"] {
            assert!(!valid_host(host), "{host:?}");
        }
        assert!(banner_fields("aida 0.16.0", 0).is_err());
        assert!(banner_fields(
            "aida 0.16.0 (built 2024-01-01 00:00:00 UTC, sha deadbeef^)",
            0
        )
        .is_err());
        assert!(banner_fields(
            "noise\naida 0.16.0 (built 2024-01-01 00:00:00 UTC, sha deadbeef)",
            0
        )
        .is_err());
    }

    #[test]
    fn lane_binaries_parses_repeated_doctor_hosts() {
        use clap::FromArgMatches;
        let args = crate::doctor_cmd::DoctorArgs::from_arg_matches(
            crate::TEST_CLI
                .built_command()
                .try_get_matches_from([
                    "aida",
                    "doctor",
                    "--lane-binaries",
                    "imac",
                    "--lane-binaries",
                    "dell1",
                    "--json",
                ])
                .unwrap()
                .subcommand_matches("doctor")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(args.lane_binaries, ["imac", "dell1"]);
        assert!(args.json);
        assert!(crate::TEST_CLI
            .built_command()
            .try_get_matches_from(["aida", "doctor", "--lane-binaries", "dell1", "--heal"])
            .is_err());
    }
}
