//! STORY-2894 S1: regenerate registered generated files at a built train's
//! tip, in a sandboxed temporary checkout, and add the one regeneration
//! commit. A refusal leaves the train exactly as built, so the worst case is
//! today's behaviour; CI then runs every ratchet on the tip.
//!
//! Generator code is always base-trusted: each generator runs from the
//! train's base, bound read-only over the tip checkout, and an entry whose
//! generator a car changes is not regenerated. The sandbox sees only that
//! checkout and a read-only toolchain: no home directory, no network.
// trace:TASK-3963 | ai:claude

use super::{commit_tree, git, git_raw, Built};
use crate::gate::train_generated::{self as tg, RegenCommit, RegenEntry};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

const DRIVER: &str = include_str!("../../scripts/repair-generated.py");
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
const OUTPUT_CAP: u64 = 1 << 20;
/// Read-only toolchain mounts; the only host paths besides the checkout.
const TOOLCHAIN: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/etc/alternatives",
    "/etc/ld.so.cache",
];
const SANDBOX_HOME: &str = "/aida-train-home";
const SANDBOX_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// The sandbox launcher and its time limit; tests replace both.
pub(crate) struct Launch {
    pub program: OsString,
    pub timeout: std::time::Duration,
}

impl Default for Launch {
    fn default() -> Self {
        Self {
            program: "bwrap".into(),
            timeout: TIMEOUT,
        }
    }
}

/// Add the regeneration commit to `train` when a selected generator changes
/// an output. A refusal is printed and recorded on the train (journaled with
/// it and named in its PR body); the train is otherwise unchanged.
pub(crate) fn regenerate(repo: &Path, project_root: &Path, train: &mut Built) {
    // Fail closed: an unreadable protected-path config disables every entry.
    let outcome = crate::gate::mandate_gates::load_human_gates(project_root)
        .map_err(|e| {
            format!(
                "the protected-path configuration cannot be read, so nothing is regenerated: {e:#}"
            )
        })
        .and_then(|gates| {
            let (entries, warnings) = tg::runnable(&gates.protected);
            for warning in warnings {
                eprintln!("warning: {warning}");
            }
            regen_commit(repo, train, &entries)
        });
    match outcome {
        Ok(Some(regen)) => {
            train.tip = regen.commit.clone();
            train.regen = Some(regen);
        }
        Ok(None) => {}
        Err(why) => {
            eprintln!("warning: train regeneration refused; the train stays as built: {why}");
            train.regen_refused = Some(tg::refusal_line(&why));
        }
    }
}

pub(crate) fn regen_commit(
    repo: &Path,
    train: &Built,
    entries: &[&'static tg::TrainEntry],
) -> Result<Option<RegenCommit>, String> {
    regen_commit_with(repo, train, entries, &Launch::default())
}

pub(crate) fn regen_commit_with(
    repo: &Path,
    train: &Built,
    entries: &[&'static tg::TrainEntry],
    launch: &Launch,
) -> Result<Option<RegenCommit>, String> {
    let mut parent = train.base.clone();
    let mut car_paths = Vec::new();
    for car in &train.cars {
        car_paths.push(touched(repo, &parent, &car.commit)?);
        parent = car.commit.clone();
    }
    let (trusted, warnings) = tg::base_trusted(entries, &car_paths);
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
    let selected = tg::select(&trusted, &car_paths);
    if selected.is_empty() || parent != train.tip {
        return Ok(None);
    }
    let Some((tree, report)) = regenerate_tree(repo, &train.base, &train.tip, &selected, launch)?
    else {
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

/// Host Git with no system or global configuration.
fn host_git(dir: &Path, index: Option<&Path>, args: &[&OsStr]) -> Result<Vec<u8>, String> {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(std::process::Stdio::null());
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`git {}`: {}",
            args.iter()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

/// A self-contained checkout of `tip` at `wt`: its own empty repository
/// whose index lists the tip, so `git ls-files` works inside the sandbox
/// without binding the host repository.
fn checkout(repo: &Path, tip: &str, wt: &Path) -> Result<(), String> {
    let format = git(repo, &["rev-parse", "--show-object-format"]).map_err(|e| format!("{e:#}"))?;
    let format = format!("--object-format={format}");
    host_git(
        repo,
        None,
        &[
            "init".as_ref(),
            "--quiet".as_ref(),
            "--template=".as_ref(),
            format.as_ref(),
            wt.as_os_str(),
        ],
    )?;
    host_git(
        repo,
        Some(&wt.join(".git/index")),
        &[
            "--work-tree".as_ref(),
            wt.as_os_str(),
            "read-tree".as_ref(),
            "-u".as_ref(),
            "--reset".as_ref(),
            tip.as_ref(),
        ],
    )?;
    Ok(())
}

/// The trusted base's copy of every generator `selected` runs, written into
/// the checkout and returned as `(read-only source, checkout path)` mounts.
fn pin_generators(
    repo: &Path,
    base: &str,
    selected: &[&str],
    dir: &Path,
    wt: &Path,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let mut mounts = Vec::new();
    for entry in tg::TRAIN_GENERATED
        .iter()
        .filter(|e| selected.contains(&e.name))
    {
        for path in entry.generators {
            let out = git_raw(repo, &["show", &format!("{base}:{path}")])
                .map_err(|e| format!("{e:#}"))?;
            if !out.status.success() {
                return Err(format!("the train base has no generator {path}"));
            }
            let pinned = dir.join(format!("generator-{}", mounts.len()));
            let target = wt.join(path);
            // No car changes it, so the tip tracks it as a regular file; never
            // write through a link out of the checkout.
            let inside = target
                .parent()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .is_some_and(|p| p.starts_with(wt.canonicalize().unwrap_or_default()));
            if !inside || !std::fs::symlink_metadata(&target).is_ok_and(|m| m.is_file()) {
                return Err(format!("the train tip's {path} is not a regular file"));
            }
            std::fs::write(&pinned, &out.stdout).map_err(|e| e.to_string())?;
            std::fs::write(&target, &out.stdout).map_err(|e| e.to_string())?;
            mounts.push((pinned, target));
        }
    }
    Ok(mounts)
}

/// The toolchain mounts and the sandbox `PATH`: the fixed read-only system
/// paths, plus the install prefix of a `python3` or `git` found on the host
/// `PATH` outside them, unless it lies in the home directory (never bound).
fn toolchain() -> (Vec<PathBuf>, OsString) {
    let env = aida_core::exec_context::Environment::capture();
    let home = env.get("HOME").and_then(|h| std::fs::canonicalize(h).ok());
    let mut mounts: Vec<PathBuf> = TOOLCHAIN.iter().map(PathBuf::from).collect();
    let mut path = OsString::from(SANDBOX_PATH);
    let search: Vec<PathBuf> = env
        .get("PATH")
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    for tool in ["python3", "git"] {
        let Some(found) = search
            .iter()
            .map(|d| d.join(tool))
            .find(|p| p.is_file())
            .and_then(|p| std::fs::canonicalize(p).ok())
        else {
            continue;
        };
        let Some(prefix) = found.parent().and_then(Path::parent) else {
            continue;
        };
        let system = TOOLCHAIN.iter().any(|t| found.starts_with(t));
        let private = home.as_ref().is_some_and(|h| found.starts_with(h));
        if system || private || prefix == Path::new("/") || mounts.iter().any(|m| m == prefix) {
            continue;
        }
        mounts.push(prefix.to_path_buf());
        path = format!(
            "{}:{}",
            found.parent().unwrap().display(),
            path.to_string_lossy()
        )
        .into();
    }
    (mounts, path)
}

/// The bwrap argv for the regeneration step. Only `wt` is writable; the
/// toolchain and pinned generators are read-only; HOME and /tmp are empty
/// tmpfs mounts; there is no network.
pub(crate) fn sandbox_args(
    wt: &Path,
    toolchain: &[PathBuf],
    path: &OsStr,
    generators: &[(PathBuf, PathBuf)],
    tip: &str,
    names: &str,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--unshare-all",
        "--unshare-net",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
    ]
    .map(OsString::from)
    .to_vec();
    for mount in toolchain {
        args.extend(["--ro-bind-try".into(), mount.into(), mount.into()]);
    }
    args.extend(
        [
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--tmpfs",
            "/tmp",
            "--tmpfs",
            SANDBOX_HOME,
        ]
        .map(OsString::from),
    );
    args.extend(["--bind".into(), wt.into(), wt.into()]);
    for (source, target) in generators {
        args.extend(["--ro-bind".into(), source.into(), target.into()]);
    }
    args.extend(["--chdir".into(), wt.into()]);
    for (key, value) in [
        ("PATH", path),
        ("HOME", SANDBOX_HOME.as_ref()),
        ("LC_ALL", "C".as_ref()),
        ("TMPDIR", "/tmp".as_ref()),
        ("PYTHONDONTWRITEBYTECODE", "1".as_ref()),
        // Never import a module from the checkout's script or working directory.
        ("PYTHONSAFEPATH", "1".as_ref()),
        ("GIT_CONFIG_NOSYSTEM", "1".as_ref()),
        ("GIT_CONFIG_GLOBAL", "/dev/null".as_ref()),
        ("GIT_OPTIONAL_LOCKS", "0".as_ref()),
    ] {
        args.extend(["--setenv".into(), key.into(), value.to_os_string()]);
    }
    args.extend(["--", "python3", "-c", DRIVER, tip, "train", names, "240"].map(OsString::from));
    args
}

fn regenerate_tree(
    repo: &Path,
    base: &str,
    tip: &str,
    selected: &[&str],
    launch: &Launch,
) -> Result<Option<(String, Vec<RegenEntry>)>, String> {
    let dir = tempfile::Builder::new()
        .prefix("aida-train-regen-")
        .tempdir()
        .map_err(|e| format!("no scratch directory: {e}"))?;
    let wt = dir.path().join("tip");
    checkout(repo, tip, &wt)?;
    let generators = pin_generators(repo, base, selected, dir.path(), &wt)?;
    let (mounts, path) = toolchain();
    let names = serde_json::to_string(selected).map_err(|e| e.to_string())?;
    let bwrap = &launch.program;
    let mut cmd = std::process::Command::new(bwrap);
    cmd.args(sandbox_args(&wt, &mounts, &path, &generators, tip, &names))
        .stdin(std::process::Stdio::null());
    let out = match crate::command_output_with_timeout_capped(cmd, launch.timeout, OUTPUT_CAP) {
        crate::BoundedCommandOutput::Completed(out) => out,
        crate::BoundedCommandOutput::TimedOut => return Err("the generators timed out".into()),
        crate::BoundedCommandOutput::SpawnFailed => {
            return Err("the regeneration sandbox could not start".into())
        }
    };
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let report: Vec<RegenEntry> =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("unreadable report: {e}"))?;
    tg::check_report(selected, &report)?;
    stage(repo, dir.path(), &wt, tip, &report)
}

fn stage(
    repo: &Path,
    dir: &Path,
    wt: &Path,
    tip: &str,
    report: &[RegenEntry],
) -> Result<Option<(String, Vec<RegenEntry>)>, String> {
    let changed = tg::changed(report);
    if changed.is_empty() {
        return Ok(None);
    }
    let index = dir.join("index");
    let staged = |args: &[&str]| -> Result<String, String> {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        host_git(repo, Some(&index), &args)
            .map(|out| String::from_utf8_lossy(&out).trim().to_string())
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
    Ok(Some((staged(&["write-tree"])?, report.to_vec())))
}
