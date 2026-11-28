//! STORY-2894 S1: regenerate registered generated files at a built train's
//! tip, in a sandboxed temporary worktree, and add the one regeneration
//! commit. A refusal leaves the train exactly as built, so the worst case is
//! today's behaviour; CI then runs every ratchet on the tip.
// trace:TASK-3963 | ai:claude

use super::{commit_tree, git, Built};
use crate::gate::train_generated::{self as tg, RegenCommit, RegenEntry};
use std::path::Path;

const DRIVER: &str = include_str!("../../scripts/repair-generated.py");
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Add the regeneration commit to `train` when a selected generator changes
/// an output. Refusals are printed; the train is then unchanged.
pub(crate) fn regenerate(repo: &Path, project_root: &Path, train: &mut Built) {
    // Read the protected set the way the mandate gates do; floor on failure.
    let protected = crate::gate::mandate_gates::load_human_gates(project_root)
        .map(|g| g.protected)
        .unwrap_or_default();
    let (entries, warnings) = tg::runnable(&protected);
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
    match regen_commit(repo, project_root, train, &entries) {
        Ok(Some(regen)) => {
            train.tip = regen.commit.clone();
            train.regen = Some(regen);
        }
        Ok(None) => {}
        Err(why) => {
            eprintln!("warning: train regeneration refused; the train stays as built: {why}")
        }
    }
}

pub(crate) fn regen_commit(
    repo: &Path,
    project_root: &Path,
    train: &Built,
    entries: &[&'static tg::TrainEntry],
) -> Result<Option<RegenCommit>, String> {
    let mut parent = train.base.clone();
    let mut car_paths = Vec::new();
    for car in &train.cars {
        car_paths.push(touched(repo, &parent, &car.commit)?);
        parent = car.commit.clone();
    }
    let selected = tg::select(entries, &car_paths);
    if selected.is_empty() || parent != train.tip {
        return Ok(None);
    }
    let Some((tree, report)) = regenerate_tree(repo, project_root, &train.tip, &selected)? else {
        return Ok(None);
    };
    let message = tg::regen_message(&report);
    let commit = commit_tree(repo, &tree, &train.tip, &message).map_err(|e| format!("{e:#}"))?;
    let regen = RegenCommit { commit, report };
    let paths = touched(repo, &train.tip, &regen.commit)?;
    tg::verify_regen(&message, &paths, &regen)?;
    ledger_check(repo, &train.tip, &regen.commit, &paths)?;
    Ok(Some(regen))
}

/// The E2 re-check of a regenerated ledger between two commits; runs no
/// generator.
pub(crate) fn ledger_check(
    repo: &Path,
    from: &str,
    to: &str,
    touched: &[String],
) -> Result<(), String> {
    let path = "scripts/store-writers.toml";
    if !touched.iter().any(|p| p == path) {
        return Ok(());
    }
    let show =
        |rev: &str| git(repo, &["show", &format!("{rev}:{path}")]).map_err(|e| format!("{e:#}"));
    tg::ledger_violation(&show(from)?, &show(to)?).map_or(Ok(()), Err)
}

/// The paths `to` changes relative to `from`.
pub(crate) fn touched(repo: &Path, from: &str, to: &str) -> Result<Vec<String>, String> {
    let args = [
        "diff-tree",
        "-r",
        "--no-commit-id",
        "--name-only",
        "-z",
        from,
        to,
    ];
    let out = git(repo, &args).map_err(|e| format!("{e:#}"))?;
    Ok(out
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect())
}

fn regenerate_tree(
    repo: &Path,
    project_root: &Path,
    tip: &str,
    selected: &[&str],
) -> Result<Option<(String, Vec<RegenEntry>)>, String> {
    let dir = tempfile::Builder::new()
        .prefix("aida-train-regen-")
        .tempdir()
        .map_err(|e| format!("no scratch directory: {e}"))?;
    let wt = dir.path().join("tip");
    let wt_s = wt.to_string_lossy().to_string();
    git(
        repo,
        &["worktree", "add", "--quiet", "--detach", &wt_s, tip],
    )
    .map_err(|e| format!("{e:#}"))?;
    let result = run_and_stage(repo, project_root, dir.path(), &wt, tip, selected);
    if let Err(e) = git(repo, &["worktree", "remove", "--force", &wt_s]) {
        eprintln!("warning: kept the train regeneration checkout {wt_s}: {e:#}");
    }
    result
}

fn run_and_stage(
    repo: &Path,
    project_root: &Path,
    dir: &Path,
    wt: &Path,
    tip: &str,
    selected: &[&str],
) -> Result<Option<(String, Vec<RegenEntry>)>, String> {
    let scratch = dir.join("tmp");
    std::fs::create_dir(&scratch).map_err(|e| e.to_string())?;
    // Offline, read-only host; only the scratch directory is writable.
    let mut cmd = std::process::Command::new("bwrap");
    cmd.args(["--unshare-all", "--die-with-parent", "--new-session"]);
    // Honour the operator's [contained] read_allowlist as conflict lanes do.
    let allowlist = crate::session::contained_read_allowlist(project_root);
    if allowlist.is_empty() {
        cmd.args(["--ro-bind", "/", "/"]);
    } else {
        let common = git(
            repo,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .map_err(|e| format!("{e:#}"))?;
        let essential = crate::session::strict_read_essential_paths()
            .iter()
            .map(|p| p.to_string());
        for path in essential.chain(allowlist).chain([common]) {
            cmd.arg("--ro-bind-try").arg(&path).arg(&path);
        }
    }
    cmd.args(["--dev", "/dev", "--proc", "/proc", "--bind"])
        .args([dir, dir])
        .arg("--chdir")
        .arg(wt)
        .arg("--clearenv");
    let path = aida_core::exec_context::Environment::capture()
        .get("PATH")
        .map_or_else(|| "/usr/bin:/bin".into(), |p| p.to_os_string());
    for (key, value) in [
        ("PATH", path),
        ("HOME", "/nonexistent".into()),
        ("LC_ALL", "C".into()),
        ("TMPDIR", scratch.into()),
        ("PYTHONDONTWRITEBYTECODE", "1".into()),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("GIT_CONFIG_GLOBAL", "/dev/null".into()),
        ("GIT_OPTIONAL_LOCKS", "0".into()),
    ] {
        cmd.arg("--setenv").arg(key).arg(value);
    }
    let names = serde_json::to_string(selected).map_err(|e| e.to_string())?;
    cmd.args(["--", "python3", "-c", DRIVER, tip, "train", &names, "240"])
        .stdin(std::process::Stdio::null());
    let out = crate::command_output_with_timeout(cmd, TIMEOUT).ok_or("the generators timed out")?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let report: Vec<RegenEntry> =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("unreadable report: {e}"))?;
    tg::check_report(selected, &report)?;
    let changed = tg::changed(&report);
    if changed.is_empty() {
        return Ok(None);
    }
    let index = dir.join("index");
    let staged = |args: &[&str]| -> Result<String, String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_INDEX_FILE", &index)
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!(
                "`git {}`: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    staged(&["read-tree", tip])?;
    for (_, output) in changed {
        let file = wt.join(&output.path);
        let meta = std::fs::symlink_metadata(&file).map_err(|e| e.to_string())?;
        let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
        use sha2::Digest as _;
        let sha = format!("{:x}", sha2::Sha256::digest(&bytes));
        let mode = staged(&["--literal-pathspecs", "ls-tree", tip, "--", &output.path])?;
        if !meta.is_file()
            || output.after.as_deref() != Some(sha.as_str())
            || !mode.starts_with("100644 blob ")
        {
            return Err(format!(
                "{} is not the reported regular 100644 file",
                output.path
            ));
        }
        let file_s = file.to_string_lossy().to_string();
        let blob = staged(&["hash-object", "-w", "--no-filters", "--", &file_s])?;
        staged(&[
            "update-index",
            "--cacheinfo",
            &format!("100644,{blob},{}", output.path),
        ])?;
    }
    Ok(Some((staged(&["write-tree"])?, report)))
}
