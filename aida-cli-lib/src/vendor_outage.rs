//! A headless agent's usage limit as a typed outage: recorded on the host
//! with its reset time, shown by `aida doctor`, and refused at launch until
//! that time passes. An expired record is simply ignored.
// trace:STORY-3134 | ai:claude
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Local, Utc};
use std::path::{Path, PathBuf};

pub(crate) const CATEGORY: &str = "vendor-outage";
/// How long an outage without a vendor-given reset time is held.
const UNKNOWN_RESET_HOLD_MINUTES: i64 = 60;
/// Only the log's tail is read; a usage-limit failure ends the run.
const TAIL_BYTES: u64 = 256 * 1024;

/// `VendorUnavailable { vendor, until }`, as recorded on the host.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Outage {
    pub(crate) vendor: String,
    pub(crate) until: DateTime<Utc>,
    /// Whether `until` came from the vendor rather than the default hold.
    pub(crate) until_known: bool,
    pub(crate) detected_at: DateTime<Utc>,
    pub(crate) message: String,
}

impl Outage {
    pub(crate) fn describe(&self) -> String {
        format!(
            "`{}` is unavailable until {}{} (usage limit: {})",
            self.vendor,
            self.until.with_timezone(&Local).format("%Y-%m-%d %H:%M %Z"),
            if self.until_known {
                ""
            } else {
                " (no reset time given; default hold)"
            },
            self.message
        )
    }
}

/// The usage-limit outage a finished run's headless log ends with, if any:
/// `classify` (the vendor's own reader) judges only the run's terminal
/// event, given the events before it; an earlier match never counts.
pub(crate) fn detect_log(
    vendor: &str,
    classify: impl Fn(
        &serde_json::Value,
        &[serde_json::Value],
    ) -> Option<crate::vendors::usage_limit::UsageLimit>,
    log: &Path,
    now: DateTime<Utc>,
) -> Option<Outage> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(log).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
        .ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let events: Vec<serde_json::Value> = String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let (terminal, earlier) = events.split_last()?;
    let limit = classify(terminal, earlier)?;
    Some(Outage {
        vendor: vendor.to_string(),
        until_known: limit.until.is_some(),
        until: limit
            .until
            .unwrap_or_else(|| now + Duration::minutes(UNKNOWN_RESET_HOLD_MINUTES)),
        detected_at: now,
        message: limit.message,
    })
}

fn dir(main: &Path) -> PathBuf {
    main.join(".aida").join("vendor-outages")
}

/// Record `outage` on the main checkout `main`, replacing any earlier one.
pub(crate) fn record(main: &Path, outage: &Outage) -> Result<()> {
    let dir = dir(main);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join(format!("{}.json", outage.vendor));
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(outage)?)
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("write {}", path.display()))
}

/// Every outage recorded on `main` that has not reset by `now`. An
/// unreadable record is skipped: an outage only ever withholds a launch.
pub(crate) fn active(main: &Path, now: DateTime<Utc>) -> Vec<Outage> {
    let mut out: Vec<Outage> = std::fs::read_dir(dir(main))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice::<Outage>(&std::fs::read(e.path()).ok()?).ok())
        .filter(|o| o.until > now)
        .collect();
    out.sort_by(|a, b| a.vendor.cmp(&b.vendor));
    out
}

/// Refuse a launch on `vendor` while it has an active outage on the main
/// checkout of `root`, before any lease or worktree exists.
pub(crate) fn refuse_launch(root: &Path, vendor: &str) -> Result<()> {
    let main = crate::main_worktree_root_from(root);
    if let Some(outage) = active(&main, Utc::now())
        .into_iter()
        .find(|o| o.vendor == vendor)
    {
        anyhow::bail!(
            "{}. Launches and reviews on `{vendor}` are refused until then; they resume on \
             their own after the reset. To retry earlier, remove {}.",
            outage.describe(),
            dir(&main).join(format!("{vendor}.json")).display()
        );
    }
    Ok(())
}

/// `aida doctor` findings: one per active outage.
pub(crate) fn scan(main: &Path) -> Vec<crate::doctor_root::DoctorFinding> {
    active(main, Utc::now())
        .into_iter()
        .map(|o| crate::doctor_root::DoctorFinding {
            category: CATEGORY.into(),
            id: o.vendor.clone(),
            summary: o.describe(),
            action: "launches on this agent are refused until the reset; it recovers on its own"
                .into(),
            safe_heal: false,
        })
        .collect()
}

fn git(tree: &Path, index: Option<&Path>, args: &[&str]) -> Result<String> {
    // The host's hardened builder; it refuses a seat clone outright.
    let mut cmd = aida_core::git_ops::host_git_command(tree);
    cmd.args(args).stdin(std::process::Stdio::null());
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let out = cmd
        .output()
        .with_context(|| format!("run git {}", args.join(" ")))?;
    anyhow::ensure!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Push `source` (a ref or commit in `repo`) to the side branch through the
/// host's remote-credential routing.
fn push_side(repo: &Path, source: &str, branch: &str) -> Result<()> {
    use aida_core::git_ops::RemoteCredentialsExt as _;
    let refspec = format!("+{source}:refs/heads/{}", side_branch(branch));
    aida_core::git_ops::clear_sandbox_config_lock(repo);
    let out = aida_core::git_ops::host_git_command(repo)
        .args(["push", "--no-verify", "-q", "origin", &refspec])
        .remote_credentials(repo, "origin")
        .stdin(std::process::Stdio::null())
        .output()
        .context("run git push")?;
    anyhow::ensure!(
        out.status.success(),
        "git push origin {refspec} failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(())
}

/// A contained seat clone's work goes the sanctioned seat transport: commit
/// and bundle inside AIDA's sandbox, verify and quarantine the bundle in
/// `main`, then push from `main`. Host git never runs in the clone.
fn preserve_seat_clone(main: &Path, clone: &Path, branch: &str) -> Result<String> {
    anyhow::ensure!(
        crate::seat_clone::host_recorded_clone(main, clone).is_some(),
        "{} is not a seat clone this checkout recorded",
        clone.display()
    );
    let message = format!("[AI:aida] chore(seat): {branch} stopped by an agent usage limit");
    crate::seat_clone::commit_pending(main, clone, branch, &message)?;
    // Nothing beyond the pinned base: no bundle can be made, nor is needed.
    if !crate::seat_clone::has_commits_beyond_base(main, clone)? {
        return Ok("its unchanged base (no work to preserve)".to_string());
    }
    let bundle = crate::seat_clone::bundle_branch(main, clone, branch)?;
    let staged = crate::ship_from_seat::stage_bundle(main, branch, &bundle, false)?;
    let pushed = push_side(main, &staged.reference, branch);
    let _ = git(main, None, &["update-ref", "-d", &staged.reference]);
    pushed.map(|()| staged.tip.clone())
}

/// The side branch a lane stopped by a usage limit is preserved on.
pub(crate) fn side_branch(branch: &str) -> String {
    format!("aida-outage/{branch}")
}

/// Push the seat's whole state (commits, staged, unstaged and untracked
/// edits) to [`side_branch`]. A plain worktree is snapshotted through a
/// private index (its index, HEAD and hooks untouched); a contained seat
/// clone takes [`preserve_seat_clone`]. Returns the pushed commit.
pub(crate) fn preserve_seat(main: &Path, tree: &Path, branch: &str) -> Result<String> {
    if aida_core::seat_clone_detect::seat_clone_root(tree).is_some() {
        return preserve_seat_clone(main, tree, branch);
    }
    let scratch = tempfile::tempdir().context("create a snapshot index")?;
    let index_path = scratch.path().join("index");
    let index = Some(index_path.as_path());
    git(tree, index, &["read-tree", "HEAD"])?;
    git(tree, index, &["add", "-A"])?;
    let snapshot = git(tree, index, &["write-tree"])?;
    let head = git(tree, None, &["rev-parse", "HEAD"])?;
    let commit = if snapshot == git(tree, None, &["rev-parse", "HEAD^{tree}"])? {
        head
    } else {
        // AIDA's own rescue commit: its identity works on a seat with none.
        let message = format!("WIP: {branch} stopped by an agent usage limit");
        let id = ["-c", "user.name=aida", "-c", "user.email=aida@localhost"];
        let args = [
            &id[..],
            &["commit-tree", &snapshot, "-p", &head, "-m", &message],
        ]
        .concat();
        git(tree, index, &args)?
    };
    push_side(tree, &commit, branch)?;
    Ok(commit)
}
