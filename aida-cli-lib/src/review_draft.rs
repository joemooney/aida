//! Data-only standalone reviewer handoff. Authority stays in the launcher.
// trace:BUG-2564 | ai:codex
use crate::gate::{review_authority as ra, review_verdict as rv, seat_authority as sa};
use anyhow::{ensure, Context, Result};
use std::os::unix::fs::MetadataExt;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::SystemTime,
};

const LIMIT: u64 = 1024 * 1024;
// The launch directory's files. trace:BUG-2707 | ai:claude
/// The parent's record of the launch: PR, head and blocking policy.
const LAUNCH_FILE: &str = "launch.json";
/// What the reviewer writes and `aida review submit-draft` validates.
const PENDING_FILE: &str = "pending.json";
/// The accepted draft, written only by the submission.
const DRAFT_FILE: &str = "draft.json";
/// The submission's acceptance receipt for `draft.json`.
const RECEIPT_FILE: &str = "submitted.json";

/// A closed schema: unknown authority fields and duplicate fields are errors.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewerDraft {
    version: u32,
    pub(crate) pr: u64,
    pub(crate) reviewed_sha: String,
    pub(crate) verdict: String,
    pub(crate) summary: String,
    pub(crate) findings: Vec<String>,
}

impl ReviewerDraft {
    /// The gate-owned view the legacy-conflict check compares.
    // trace:BUG-2564 | ai:claude
    pub(crate) fn facts(&self) -> crate::gate::review_verdict::DraftFacts<'_> {
        crate::gate::review_verdict::DraftFacts {
            pr: self.pr,
            verdict: &self.verdict,
            reviewed_sha: &self.reviewed_sha,
            findings: &self.findings,
        }
    }

    pub(crate) fn record_findings(&self) -> Vec<String> {
        if self.findings.is_empty()
            && rv::VerdictKind::parse(&self.verdict) != rv::VerdictKind::Waiting
        {
            vec![String::new()]
        } else {
            self.findings.clone()
        }
    }
}

/// The parent retains this directory descriptor across the vendor execution.
/// A new UUID directory makes an old launch's draft ineligible on resume.
pub(crate) struct LaunchDraft {
    dir: File,
    path: PathBuf,
    pr: u64,
    head: String,
    since: SystemTime,
}

impl LaunchDraft {
    /// `policy` is the launching (parent) project's blocking policy. It is
    /// written into the launch directory so the reviewer's submission is
    /// decided under it wherever the reviewer runs.
    // trace:BUG-2707 | ai:claude
    pub(crate) fn prepare(
        worktree: &Path,
        pr: u64,
        head: String,
        policy: &crate::gate::kernel_deps::review_blocking::Effective,
    ) -> Result<Self> {
        ensure!(full_sha(&head), "review launch requires a full head SHA");
        let root = worktree.canonicalize()?.join(".aida/review-verdicts");
        std::fs::create_dir_all(&root)?;
        // Pin the verdict root before creating the exclusive child directory.
        let parent = rv::open_verdict_directory(&root)?;
        let name = format!("draft-{}", uuid::Uuid::new_v4());
        use std::os::fd::AsRawFd;
        let cname = std::ffi::CString::new(name.as_bytes())?;
        ensure!(
            unsafe { libc::mkdirat(parent.as_raw_fd(), cname.as_ptr(), 0o700) } == 0,
            "cannot create exclusive review draft directory: {}",
            std::io::Error::last_os_error()
        );
        let dir = rv::open_verdict_entry(&parent, std::ffi::OsStr::new(&name), libc::O_DIRECTORY)?;
        let record = serde_json::json!({"version": 1, "pr": pr, "head": head,
            "policy": crate::gate::kernel_deps::review_blocking::to_snapshot(policy)});
        write_in(&dir, LAUNCH_FILE, record.to_string().as_bytes())
            .context("cannot write the review launch record")?;
        // Use the same filesystem clock as the future draft, rather than
        // wall time: cached kernel timestamps can lag a just-read clock.
        let since = dir.metadata()?.modified()?;
        Ok(Self {
            dir,
            path: root.join(name).join(DRAFT_FILE),
            pr,
            head,
            since,
        })
    }

    /// A launch whose parent project is the review tree itself.
    #[cfg(test)]
    pub(crate) fn prepare_local(worktree: &Path, pr: u64, head: String) -> Result<Self> {
        let policy = crate::gate::kernel_deps::review_blocking::effective(worktree);
        Self::prepare(worktree, pr, head, &policy)
    }

    /// The launch directory: the one argument of `aida review submit-draft`.
    fn launch_dir(&self) -> &Path {
        self.path.parent().unwrap_or(&self.path)
    }

    /// Where the reviewer writes its draft before submitting it.
    pub(crate) fn pending_path(&self) -> PathBuf {
        self.launch_dir().join(PENDING_FILE)
    }

    /// The command that submits the pending draft, quoted for a shell.
    // trace:BUG-2707 | ai:claude
    pub(crate) fn submit_command(&self) -> String {
        format!(
            "aida review submit-draft {}",
            aida_core::work_binding::shell_quote(&self.launch_dir().to_string_lossy())
        )
    }

    // trace:BUG-2707 | ai:claude
    pub(crate) fn prompt(&self, prompt: &str) -> String {
        format!(
            "{prompt}\n\nStandalone reviewer handoff (supersedes in-sandbox recording), on fresh and resumed reviews alike: write your verdict as UTF-8 JSON data to the absolute path `{pending}`, then submit it by running this command exactly as written: {command}\nThat command is the only supported submission. It validates the draft and, when it refuses, says what to change: correct `{PENDING_FILE}` and run it again. A draft it has not accepted is not a submission and the parent records nothing from it; writing `{DRAFT_FILE}` yourself is refused. Do not run `aida review record`. If a legacy record attempt was refused, still submit this draft. The parent validates the accepted draft again and records it under this launch's reviewer grant. Contract: {{\"version\":1,\"pr\":{pr},\"reviewed_sha\":\"{head}\",\"verdict\":\"approved|request-changes|rejected|waiting\",\"summary\":\"complete summary\",\"findings\":[\"complete finding text\"]}}. Include every finding. Any unresolved finding means request-changes: an approved draft may carry only findings written as deferred follow-ups, and the submission refuses any other approved draft. No additional fields, recorder identity, grant, attestation, receipt, covered specs, commands or ledger paths. Review exactly the named full SHA. Exit only after the submission command has accepted the draft; then give a brief final response. Never claim the parent has recorded it.",
            pending = self.pending_path().display(),
            command = self.submit_command(),
            pr = self.pr,
            head = self.head,
        )
    }

    pub(crate) fn read(&self) -> Result<ReviewerDraft> {
        let file = match rv::open_verdict_entry(&self.dir, std::ffi::OsStr::new(DRAFT_FILE), 0) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // A draft the reviewer wrote but never got accepted is a
                // refused handoff, not an absent one. trace:BUG-2707 | ai:claude
                let pending =
                    rv::open_verdict_entry(&self.dir, std::ffi::OsStr::new(PENDING_FILE), 0);
                ensure!(
                    pending.is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
                    "rejected reviewer draft: {} was never accepted by `aida review submit-draft`, so nothing is recorded from it",
                    self.pending_path().display()
                );
                return Err(e).context("missing or unreadable reviewer draft");
            }
            Err(e) => return Err(e).context("missing or unreadable reviewer draft"),
        };
        let (body, meta) = read_bounded(file)?;
        ensure!(
            meta.modified()? >= self.since,
            "rejected stale launch draft"
        );
        // Only the submission command writes the acceptance receipt: a draft
        // file written directly was never validated while the reviewer ran.
        // trace:BUG-2707 | ai:claude
        let receipt = rv::open_verdict_entry(&self.dir, std::ffi::OsStr::new(RECEIPT_FILE), 0)
            .ok()
            .and_then(|file| read_bounded(file).ok())
            .and_then(|(body, _)| serde_json::from_str::<serde_json::Value>(&body).ok());
        ensure!(
            receipt
                .as_ref()
                .and_then(|r| r.get("sha256"))
                .and_then(|v| v.as_str())
                == Some(digest(&body).as_str()),
            "rejected reviewer draft: {} was written directly, not accepted by `aida review submit-draft`; a raw file write is not a submission",
            self.path.display()
        );
        let draft = parse_draft(&body)?;
        ensure!(draft.pr == self.pr, "reviewer draft version or PR mismatch");
        ensure!(
            draft.reviewed_sha == self.head,
            "reviewer draft does not match the full launch head"
        );
        Ok(draft)
    }

    /// Place `body` as an accepted draft without validating it, to exercise
    /// what the parent's own read refuses.
    #[cfg(test)]
    pub(crate) fn place(&self, body: &str) {
        std::fs::write(&self.path, body).unwrap();
        self.stamp();
    }

    /// Write the acceptance receipt for whatever `draft.json` now holds.
    #[cfg(test)]
    pub(crate) fn stamp(&self) {
        let body = std::fs::read_to_string(&self.path).unwrap();
        std::fs::write(self.path.with_file_name(RECEIPT_FILE), receipt_for(&body)).unwrap();
    }

    /// Write `body` as the pending draft and submit it, as a reviewer does.
    #[cfg(test)]
    pub(crate) fn submit(&self, body: &str) -> Result<ReviewerDraft> {
        std::fs::write(self.pending_path(), body).unwrap();
        submit_draft(self.launch_dir())
    }

    pub(crate) fn validate_current_head(&self, current: Option<&str>) -> Result<()> {
        ensure!(
            current == Some(self.head.as_str()),
            "review head advanced or is missing; a fresh review is required"
        );
        Ok(())
    }

    /// Mark consumed only after the canonical ledger and collector succeeded.
    /// Both names are relative to the descriptor held since before spawn.
    pub(crate) fn consumed(&self) -> Result<()> {
        use std::os::fd::AsRawFd;
        let from = c"draft.json"; // DRAFT_FILE
        let to = c"recorded.json";
        let rc = unsafe {
            libc::renameat(
                self.dir.as_raw_fd(),
                from.as_ptr(),
                self.dir.as_raw_fd(),
                to.as_ptr(),
            )
        };
        if rc != 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(e.into());
            }
        }
        Ok(())
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// The unconsumed reviewer handoffs (`draft-*/draft.json`) a review checkout
/// still holds. A recorded draft is renamed to `recorded.json`, so any
/// `draft.json` left here is a draft the parent never recorded (a failed
/// recording or a rejected draft) and is the only copy of its findings.
/// Teardown keeps the checkout while this is non-empty, and treats an
/// unreadable verdict directory (`Err`) the same way.
// trace:BUG-2684 | ai:claude
pub(crate) fn unconsumed_drafts(worktree: &Path) -> Result<Vec<PathBuf>> {
    // Report the same canonical path `LaunchDraft::prepare` handed the reviewer.
    let root = worktree
        .canonicalize()
        .unwrap_or_else(|_| worktree.to_path_buf())
        .join(".aida/review-verdicts");
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("cannot inspect {}", root.display())),
    };
    let mut drafts = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("cannot inspect {}", root.display()))?;
        if !entry.file_name().to_string_lossy().starts_with("draft-") {
            continue;
        }
        // A pending draft the submission never accepted holds the review's
        // findings just as an unrecorded accepted one does.
        // trace:BUG-2707 | ai:claude
        // Once the launch is recorded, a leftover pending file is a
        // superseded attempt.
        let recorded = std::fs::symlink_metadata(entry.path().join("recorded.json")).is_ok();
        for name in [DRAFT_FILE, PENDING_FILE] {
            if name == PENDING_FILE && recorded {
                continue;
            }
            let draft = entry.path().join(name);
            match std::fs::symlink_metadata(&draft) {
                Ok(_) => drafts.push(draft),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e).with_context(|| format!("cannot inspect {}", draft.display()))
                }
            }
        }
    }
    drafts.sort();
    Ok(drafts)
}

/// `Some(why kept)` when teardown of `worktree` would discard an unrecorded
/// reviewer draft (or the drafts cannot be inspected); `None` when it holds
/// none. trace:BUG-2684 | ai:claude
pub(crate) fn unconsumed_draft_retention(worktree: &Path) -> Option<String> {
    match unconsumed_drafts(worktree) {
        Ok(drafts) if drafts.is_empty() => None,
        Ok(drafts) => Some(format!(
            "it holds an unrecorded reviewer draft at {}; record it (or remove it deliberately) before the checkout can be removed",
            drafts
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Err(e) => Some(format!("its reviewer drafts could not be inspected ({e:#})")),
    }
}

/// The closed-schema checks every draft passes, wherever it is read: the
/// launcher's collection and the reviewer's own submission share them.
// trace:BUG-2707 | ai:claude
fn parse_draft(body: &str) -> Result<ReviewerDraft> {
    let draft: ReviewerDraft =
        serde_json::from_str(body).context("rejected malformed reviewer draft")?;
    ensure!(draft.version == 1, "reviewer draft version or PR mismatch");
    ensure!(
        full_sha(&draft.reviewed_sha),
        "reviewer draft does not match the full launch head"
    );
    ensure!(
        !draft.summary.trim().is_empty(),
        "reviewer draft requires a complete summary"
    );
    ensure!(
        rv::VerdictKind::parse(&draft.verdict) != rv::VerdictKind::Unknown,
        "unrecognized reviewer draft verdict"
    );
    ensure!(
        rv::VerdictKind::parse(&draft.verdict) != rv::VerdictKind::Waiting
            || draft.findings.is_empty(),
        "waiting cannot carry substantive findings"
    );
    ensure!(
        draft.findings.iter().all(|f| !f.trim().is_empty()),
        "empty draft finding"
    );
    Ok(draft)
}

/// Read an already opened file after checking the handle itself: a bounded,
/// singly linked regular file. Nothing is looked up by path again, so the
/// file cannot be swapped or grown between the check and the read.
// trace:BUG-2707 | ai:claude
fn read_bounded(file: File) -> Result<(String, std::fs::Metadata)> {
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.nlink() == 1 && meta.len() <= LIMIT,
        "rejected reviewer draft: expected bounded singly linked regular file"
    );
    let mut body = String::new();
    file.take(LIMIT + 1).read_to_string(&mut body)?;
    ensure!(
        body.len() as u64 <= LIMIT,
        "reviewer draft exceeds size limit"
    );
    Ok((body, meta))
}

fn digest(body: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(body.as_bytes()))
}

fn receipt_for(body: &str) -> String {
    serde_json::json!({"version": 1, "sha256": digest(body)}).to_string()
}

/// Atomically (re)place `name` inside the held directory `dir`.
// trace:BUG-2707 | ai:claude
fn write_in(dir: &File, name: &str, body: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    let tmp = std::ffi::CString::new(format!(".{name}.tmp"))?;
    let to = std::ffi::CString::new(name)?;
    // SAFETY: `dir` stays open across each call and both names are
    // NUL-terminated; the created descriptor is owned exactly once by `File`.
    unsafe { libc::unlinkat(dir.as_raw_fd(), tmp.as_ptr(), 0) };
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            tmp.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600 as libc::c_uint,
        )
    };
    ensure!(
        fd >= 0,
        "cannot create {name}: {}",
        std::io::Error::last_os_error()
    );
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(body)?;
    file.sync_all()?;
    ensure!(
        unsafe { libc::renameat(dir.as_raw_fd(), tmp.as_ptr(), dir.as_raw_fd(), to.as_ptr()) } == 0,
        "cannot place {name}: {}",
        std::io::Error::last_os_error()
    );
    Ok(())
}

fn remove_in(dir: &File, name: &str) {
    use std::os::fd::AsRawFd;
    if let Ok(name) = std::ffi::CString::new(name) {
        // SAFETY: `dir` is open and the name is NUL-terminated.
        unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) };
    }
}

/// The supported draft submission: validate the pending draft in
/// `launch_dir` the way the launcher and the recorder will, under the
/// launching project's policy recorded there, and accept it only when it
/// passes. Acceptance places `draft.json` with its receipt; a refusal changes
/// nothing and says what to correct. Nothing here depends on the working
/// directory or on the checkout the command runs from.
// trace:BUG-2707 | ai:claude
pub(crate) fn submit_draft(launch_dir: &Path) -> Result<ReviewerDraft> {
    use crate::gate::kernel_deps::review_blocking as rb;
    let name = |n: &str| std::ffi::OsString::from(n);
    let dir = rv::open_verdict_directory(launch_dir).with_context(|| {
        format!(
            "no review launch directory at {}: pass the directory the launch prompt named",
            launch_dir.display()
        )
    })?;
    let launch = rv::open_verdict_entry(&dir, &name(LAUNCH_FILE), 0)
        .map_err(anyhow::Error::from)
        .and_then(read_bounded)
        .and_then(|(body, _)| Ok(serde_json::from_str::<serde_json::Value>(&body)?))
        .with_context(|| {
            format!(
                "{} is not a review launch directory (no readable {LAUNCH_FILE}): pass the directory the launch prompt named",
                launch_dir.display()
            )
        })?;
    let (Some(pr), Some(head)) = (
        launch.get("pr").and_then(|v| v.as_u64()),
        launch.get("head").and_then(|v| v.as_str()),
    ) else {
        anyhow::bail!(
            "the review launch record in {} is unreadable",
            launch_dir.display()
        );
    };
    // Absent or unrecognised policy data fails closed.
    let effective = rb::from_snapshot(launch.get("policy").unwrap_or(&serde_json::Value::Null));
    let pending = launch_dir.join(PENDING_FILE);
    let file = rv::open_verdict_entry(&dir, &name(PENDING_FILE), 0).with_context(|| {
        format!(
            "no draft to submit at {}: write the draft JSON there, then run this command again",
            pending.display()
        )
    })?;
    let (body, _) = read_bounded(file)?;
    let draft = parse_draft(&body)?;
    ensure!(
        draft.pr == pr,
        "reviewer draft names PR {}, but this launch reviews PR {pr}: set \"pr\" to {pr}",
        draft.pr
    );
    ensure!(
        draft.reviewed_sha == head,
        "reviewer draft names reviewed_sha {}, but this launch reviews {head}: review that commit and set \"reviewed_sha\" to it",
        draft.reviewed_sha
    );
    if let Some(why) = rv::approval_refusal(&effective, &draft.verdict, &draft.record_findings()) {
        anyhow::bail!(why);
    }
    // Withdraw any earlier acceptance before replacing the draft, so a
    // receipt never vouches for bytes it did not validate.
    remove_in(&dir, RECEIPT_FILE);
    write_in(&dir, DRAFT_FILE, body.as_bytes())?;
    write_in(&dir, RECEIPT_FILE, receipt_for(&body).as_bytes())?;
    remove_in(&dir, PENDING_FILE);
    Ok(draft)
}

/// `aida review submit-draft`: a standalone reviewer submits its draft.
// trace:BUG-2707 | ai:claude
pub(crate) fn handle_submit_draft(launch_dir: &Path) -> Result<()> {
    let draft = submit_draft(launch_dir)?;
    println!(
        "{} reviewer draft accepted: {} with {} finding{}; the launcher records it after you exit",
        crate::glyph(crate::glyphs::Glyph::Check),
        draft.verdict,
        draft.findings.len(),
        if draft.findings.len() == 1 { "" } else { "s" },
    );
    Ok(())
}

fn full_sha(sha: &str) -> bool {
    sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Constructed only from the grant returned by the parent-side spawn, never
/// from child data. The grant's unique session is the recorder identity;
/// its process/key binding is retained separately as presentation evidence.
pub(crate) struct VerifiedReviewer {
    grant: sa::SeatGrant,
    launcher: String,
    session: String,
    handoff: Option<(String, String)>,
}

impl VerifiedReviewer {
    pub(crate) fn from_launch(
        root: &Path,
        child: &str,
        launcher: &str,
        since: SystemTime,
        session: &str,
    ) -> Result<Self> {
        let grant = sa::grant_check_for(root, child, &crate::current_user_id(None))
            .into_valid()
            .context("reviewer child grant is invalid")?;
        ensure!(
            SystemTime::from(grant.issued_at) >= since,
            "stale launch reviewer grant"
        );
        let result = Self {
            grant,
            launcher: launcher.to_string(),
            session: session.to_string(),
            handoff: None,
        };
        result.revalidate(root)?;
        Ok(result)
    }

    pub(crate) fn with_draft(
        mut self,
        launch: &LaunchDraft,
        draft: &ReviewerDraft,
    ) -> Result<Self> {
        use sha2::{Digest, Sha256};
        ensure!(
            draft.pr == launch.pr && draft.reviewed_sha == launch.head,
            "draft launch mismatch"
        );
        let id = launch
            .path
            .parent()
            .and_then(Path::file_name)
            .context("missing draft launch identity")?
            .to_string_lossy()
            .to_string();
        let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(draft)?));
        self.handoff = Some((id, digest));
        Ok(self)
    }

    /// This launch's handoff identity for the legacy-conflict check: its
    /// draft launch stamp and its reviewer grant.
    // trace:BUG-2564 | ai:claude
    pub(crate) fn own_handoff(&self) -> Option<rv::OwnHandoff<'_>> {
        self.handoff.as_ref().map(|(launch, _)| rv::OwnHandoff {
            launch,
            grant: &self.grant.id,
        })
    }

    pub(crate) fn handoff_stamp(&self) -> Option<serde_json::Value> {
        self.handoff
            .as_ref()
            .map(|(id, digest)| serde_json::json!({"version":1,"launch":id,"sha256":digest}))
    }

    pub(crate) fn check_replay(&self, root: &Path, key: &str) -> Result<Option<rv::JsonObj>> {
        let Some((id, digest)) = &self.handoff else {
            return Ok(None);
        };
        let current = rv::read_current_verdict_object(&rv::verdict_path(root, key))?;
        let current_id = current
            .get("reviewer_draft")
            .and_then(|v| v.get("launch"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let change =
            crate::gate::ledger::ChangeKey::parse(key).context("draft must name a change")?;
        let records = crate::gate::ledger::project::change_verdict_records_checked(root, change)
            .map_err(anyhow::Error::msg)?;
        let mut recorded = false;
        for record in records {
            if let Some(stamp) = record
                .get("reviewer_draft")
                .filter(|v| v.get("launch").and_then(|v| v.as_str()) == Some(id.as_str()))
            {
                ensure!(
                    stamp.get("sha256").and_then(|v| v.as_str()) == Some(digest.as_str()),
                    "this launch already recorded a different draft"
                );
                recorded = true;
                ensure!(
                    current_id.as_deref() == Some(id.as_str()),
                    "this draft was already recorded and a newer review exists; refusing replay"
                );
            }
        }
        Ok(recorded.then_some(current))
    }

    pub(crate) fn revalidate(&self, root: &Path) -> Result<()> {
        let grant = sa::grant_check_for(root, &self.grant.id, &crate::current_user_id(None))
            .into_valid()
            .context("reviewer child grant expired, revoked or invalid")?;
        ensure!(
            grant.seat == "reviewer"
                && grant.id != self.launcher
                && grant.parent_grant_id.as_deref() == Some(self.launcher.as_str()),
            "reviewer grant is not this launcher's independent reviewer child"
        );
        ensure!(
            !self.session.trim().is_empty(),
            "missing launched reviewer session identity"
        );
        ensure!(
            !grant.session_id.is_empty()
                && grant.session_id == self.grant.session_id
                && grant.binding == self.grant.binding
                && grant.subject == self.grant.subject,
            "reviewer grant binding changed"
        );
        ensure!(
            sa::presenter_rejection(&grant, &sa::presenter_tokens_for(&grant)).is_none(),
            "launcher cannot present the issued reviewer grant"
        );
        let launcher = sa::current_grant(root).context("launcher's grant is invalid")?;
        ensure!(
            launcher.id == self.launcher && launcher.session_id != grant.session_id,
            "launcher's authority changed"
        );
        let identity = self.identity();
        let current = ra::RecorderIdentity::current(&ra::visible_leases(root));
        ensure!(
            !ra::identities_overlap(&identity.tokens, &current.tokens),
            "reviewer identity overlaps launcher identity"
        );
        Ok(())
    }

    pub(crate) fn identity(&self) -> ra::RecorderIdentity {
        ra::RecorderIdentity {
            tokens: vec![
                format!("session:{}", self.session),
                format!("session:{}", self.grant.session_id),
            ],
        }
    }
    pub(crate) fn grant(&self) -> &sa::SeatGrant {
        &self.grant
    }
    pub(crate) fn recorded_by(&self) -> String {
        format!("reviewer:{}", self.grant.session_id)
    }
}

// Bounded same-launch attempts for recording a validated draft under the
// launch's own reviewer grant. trace:BUG-2564 | ai:claude
const MAX_HANDOFF_ATTEMPTS: u32 = 3;

/// Bounded same-launch attempts for recording a validated draft.
pub(crate) fn handoff_attempts() -> u32 {
    MAX_HANDOFF_ATTEMPTS
}

/// The wait before the given retry (1-based attempt that just failed).
pub(crate) fn handoff_backoff(attempt: u32) -> std::time::Duration {
    if cfg!(test) {
        return std::time::Duration::ZERO;
    }
    std::time::Duration::from_secs(if attempt <= 1 { 2 } else { 5 })
}

#[cfg(test)]
mod standalone_review_verdict_draft_tests {
    use super::*;
    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
    fn payload() -> serde_json::Value {
        serde_json::json!({"version":1,"pr":59,"reviewed_sha":HEAD,
            "verdict":"request-changes","summary":"complete rationale","findings":["blocking defect"]})
    }
    fn fixture() -> (tempfile::TempDir, LaunchDraft) {
        let tmp = tempfile::tempdir().unwrap();
        let launch = LaunchDraft::prepare_local(tmp.path(), 59, HEAD.into()).unwrap();
        (tmp, launch)
    }
    fn write(launch: &LaunchDraft, value: &serde_json::Value) {
        launch.place(&value.to_string());
    }
    #[test]
    fn standalone_review_verdict_draft_preserves_complete_data_and_prompt_path() {
        let (_tmp, launch) = fixture();
        write(&launch, &payload());
        let draft = launch.read().unwrap();
        assert_eq!(draft.findings, ["blocking defect"]);
        assert_eq!(draft.summary, "complete rationale");
        assert_eq!(draft.reviewed_sha, HEAD);
        for prompt in ["fresh review", "resumed review"] {
            let rendered = launch.prompt(prompt);
            assert!(rendered.contains(launch.pending_path().to_str().unwrap()));
            assert!(rendered.contains("Do not run `aida review record`"));
            assert!(rendered.contains("still submit this draft"));
        }
    }
    // trace:BUG-2707 | ai:claude
    fn policy(text: &str) -> crate::gate::kernel_deps::review_blocking::Effective {
        crate::gate::kernel_deps::review_blocking::from_config_text(text)
    }
    const TIERS: &str = "[review]\nblocking_tiers = [\"p1\", \"p2\"]\n";
    /// A launch whose parent project runs under `config`.
    fn launch_under(config: &str) -> (tempfile::TempDir, LaunchDraft) {
        let tmp = tempfile::tempdir().unwrap();
        let launch = LaunchDraft::prepare(tmp.path(), 59, HEAD.into(), &policy(config)).unwrap();
        (tmp, launch)
    }
    fn approved(findings: serde_json::Value) -> serde_json::Value {
        let mut value = payload();
        value["verdict"] = "approved".into();
        value["findings"] = findings;
        value
    }
    fn finding(tier: &str, extra: serde_json::Value) -> String {
        let mut detail = serde_json::json!({"text": format!("{tier} observation"),
            "severity": "minor", "blocking_tier": tier});
        for (k, v) in extra.as_object().unwrap() {
            detail[k] = v.clone();
        }
        detail.to_string()
    }
    fn refusal(launch: &LaunchDraft, value: &serde_json::Value) -> String {
        format!("{:#}", launch.submit(&value.to_string()).unwrap_err())
    }
    /// The 2026-11-20 shape (#4364, #4377): APPROVED carrying p3 and style
    /// findings as plain follow-up notes. The submission refuses it while the
    /// reviewer still runs, names the findings and says what to change;
    /// nothing is accepted, so the parent's read refuses too. The corrected
    /// draft is then accepted through the same entry point.
    // trace:BUG-2707 | ai:claude
    #[test]
    fn bug_2707_approved_draft_with_unresolved_findings_is_refused_at_submission() {
        for (name, findings, config) in [
            (
                "plain text",
                serde_json::json!(["P3: rename the helper", "style: wrap the long line"]),
                "",
            ),
            (
                "tiered but no deferral",
                serde_json::json!([
                    finding("p3", serde_json::json!({})),
                    finding("style", serde_json::json!({}))
                ]),
                "",
            ),
            (
                "deferral without a rationale",
                serde_json::json!([finding("p3", serde_json::json!({"disposition":"deferred"}))]),
                "",
            ),
            (
                "plain text under a list",
                serde_json::json!(["P3: rename the helper", "style: wrap the long line"]),
                TIERS,
            ),
            (
                "listed class under a list",
                serde_json::json!([
                    finding("style", serde_json::json!({})),
                    finding(
                        "p2",
                        serde_json::json!({"disposition":"deferred","rationale":"later"})
                    )
                ]),
                TIERS,
            ),
        ] {
            let (tmp, launch) = launch_under(config);
            let draft = approved(findings);
            let err = refusal(&launch, &draft);
            assert!(err.contains("reviewer draft refused"), "{name}: {err}");
            assert!(err.contains("unresolved finding"), "{name}: {err}");
            assert!(err.contains("\"request-changes\""), "{name}: {err}");
            assert!(err.contains("Never drop a finding"), "{name}: {err}");
            // Nothing was accepted: the parent has no submission to record,
            // and the refused draft is kept for whoever recovers the review.
            assert!(!launch.path.exists(), "{name}");
            let read = format!("{:#}", launch.read().unwrap_err());
            assert!(read.contains("never accepted"), "{name}: {read}");
            assert_eq!(
                unconsumed_drafts(tmp.path()).unwrap(),
                [launch.pending_path()],
                "{name}"
            );
            // The reviewer corrects the draft and resubmits through the same
            // operation, which now accepts it.
            let mut fixed = draft.clone();
            fixed["verdict"] = "request-changes".into();
            let accepted = launch.submit(&fixed.to_string()).unwrap();
            assert_eq!(accepted.verdict, "request-changes", "{name}");
            let read = launch.read().unwrap();
            assert_eq!(read.findings, accepted.findings, "{name}");
            assert_eq!(
                unconsumed_drafts(tmp.path()).unwrap(),
                [launch.path.clone()],
                "{name}"
            );
        }
        // Only the finding the recorder would not defer is named.
        let (_tmp, launch) = launch_under(TIERS);
        let err = refusal(
            &launch,
            &approved(serde_json::json!([
                finding("style", serde_json::json!({})),
                finding("p2", serde_json::json!({}))
            ])),
        );
        assert!(
            err.contains("1 unresolved finding (#2 \"p2 observation\": in blocking class p2)"),
            "{err}"
        );
    }
    /// A raw write of the contradictory approval is not a submission: with
    /// no acceptance, the parent's read refuses it, and a receipt for other
    /// bytes does not vouch for it either.
    // trace:BUG-2707 | ai:claude
    #[test]
    fn bug_2707_raw_draft_write_is_not_a_submission() {
        let (_tmp, launch) = launch_under("");
        let contradictory = approved(serde_json::json!(["P3: rename the helper"])).to_string();
        std::fs::write(&launch.path, &contradictory).unwrap();
        let err = format!("{:#}", launch.read().unwrap_err());
        assert!(err.contains("written directly"), "{err}");
        assert!(err.contains("aida review submit-draft"), "{err}");
        // An accepted submission followed by a raw overwrite is refused too.
        let (_tmp, launch) = launch_under("");
        launch.submit(&payload().to_string()).unwrap();
        assert!(launch.read().is_ok());
        std::fs::write(&launch.path, &contradictory).unwrap();
        let err = format!("{:#}", launch.read().unwrap_err());
        assert!(err.contains("written directly"), "{err}");
        // A refused resubmission accepts nothing new: the accepted draft
        // stays the one last validated.
        let (_tmp, launch) = launch_under("");
        launch.submit(&payload().to_string()).unwrap();
        assert!(launch.submit(&contradictory).is_err());
        assert_eq!(launch.read().unwrap().verdict, "request-changes");
    }
    /// The default-policy remedy states the rule the recorder applies: an
    /// explicit deferral needs severity, disposition and a one-line
    /// rationale, and no `blocking_tier`. The skill's own example is accepted.
    // trace:BUG-2707 | ai:claude
    #[test]
    fn bug_2707_default_policy_remedy_matches_the_recorder_rule() {
        let (_tmp, launch) = launch_under("");
        let err = refusal(
            &launch,
            &approved(serde_json::json!(["P3: rename the helper"])),
        );
        assert!(err.contains("\"disposition\":\"deferred\""), "{err}");
        assert!(err.contains("\"rationale\""), "{err}");
        assert!(!err.contains("blocking_tier"), "{err}");
        let example = r#"{"text":"Document the fallback","severity":"cosmetic","disposition":"deferred","rationale":"The implemented behavior is correct and tested"}"#;
        let skill = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../aida-core/templates/skills/aida-review.md"),
        )
        .unwrap();
        assert!(
            skill.contains(example),
            "the skill no longer shows this example"
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(example).unwrap()["blocking_tier"].is_null()
        );
        let accepted = launch
            .submit(&approved(serde_json::json!([example])).to_string())
            .unwrap();
        assert_eq!(accepted.verdict, "approved");
        // A multi-line rationale is not a deferral, exactly as the recorder rules.
        let multi = serde_json::json!({"text":"t","severity":"minor","disposition":"deferred","rationale":"a\nb"}).to_string();
        assert!(launch
            .submit(&approved(serde_json::json!([multi])).to_string())
            .is_err());
    }
    /// Under a configured list, findings that block whatever the list says
    /// are named as such and never offered restructuring: request-changes is
    /// the only remedy. Restructuring is offered only for a finding with no
    /// registry tier.
    // trace:BUG-2707 | ai:claude
    #[test]
    fn bug_2707_intrinsically_blocking_findings_get_request_changes_only() {
        // Neither security nor test-gap is on this list.
        let config = "[review]\nblocking_tiers = [\"p1\"]\n";
        for (name, extra_tier, extra, what) in [
            (
                "security tier",
                "security-authority-bypass",
                serde_json::json!({}),
                "security or authority-bypass",
            ),
            (
                "security class on a p3",
                "p3",
                serde_json::json!({"class": "security"}),
                "security or authority-bypass",
            ),
            (
                "test gap in gate code",
                "test-gap",
                serde_json::json!({"file": "aida-cli-lib/src/gate/ship.rs"}),
                "test gap in gate",
            ),
            (
                "ADR-76 spelling",
                "mutation-kill",
                serde_json::json!({}),
                "test gap in gate",
            ),
        ] {
            let (_tmp, launch) = launch_under(config);
            let err = refusal(
                &launch,
                &approved(serde_json::json!([finding(extra_tier, extra)])),
            );
            assert!(err.contains("always blocking"), "{name}: {err}");
            assert!(err.contains(what), "{name}: {err}");
            assert!(
                err.contains("request-changes is the only remedy"),
                "{name}: {err}"
            );
            assert!(!err.contains("(b)"), "{name}: {err}");
            assert!(!err.contains("write it as"), "{name}: {err}");
        }
        // A listed class alone is not restructurable either.
        let (_tmp, launch) = launch_under(config);
        let err = refusal(
            &launch,
            &approved(serde_json::json!([finding("p1", serde_json::json!({}))])),
        );
        assert!(err.contains("in blocking class p1"), "{err}");
        assert!(!err.contains("(b)"), "{err}");
        // Mixed with an unclassified finding: restructuring is offered for
        // that one only, and the draft still needs request-changes.
        let err = refusal(
            &launch,
            &approved(serde_json::json!([
                finding("security", serde_json::json!({})),
                "style: wrap the long line"
            ])),
        );
        assert!(
            err.contains("#1 \"security observation\": always blocking"),
            "{err}"
        );
        assert!(
            err.contains("#2 \"style: wrap the long line\": no registry \"blocking_tier\""),
            "{err}"
        );
        assert!(
            err.contains("(b) only for a finding marked `no registry"),
            "{err}"
        );
        assert!(err.contains("request-changes is the only remedy"), "{err}");
        // An ordinary test gap outside sensitive code still defers.
        let (_tmp, launch) = launch_under(config);
        launch
            .submit(
                &approved(serde_json::json!([finding(
                    "test-gap",
                    serde_json::json!({"file": "docs/guide.md"})
                )]))
                .to_string(),
            )
            .unwrap();
    }
    /// The submission agrees with the recorder on everything it accepts.
    // trace:BUG-2707 | ai:claude
    #[test]
    fn bug_2707_consistent_drafts_are_accepted() {
        let deferred = serde_json::json!({"disposition":"deferred","rationale":"cosmetic; a follow-up is enough"});
        for (name, value, config) in [
            ("approved, no findings", approved(serde_json::json!([])), ""),
            (
                "approved, explicit deferrals",
                approved(serde_json::json!([
                    finding("p3", deferred.clone()),
                    finding("style", deferred.clone())
                ])),
                "",
            ),
            (
                "approved, below the list",
                approved(serde_json::json!([
                    finding("p3", serde_json::json!({})),
                    finding("style", serde_json::json!({}))
                ])),
                TIERS,
            ),
            ("request-changes, plain findings", payload(), ""),
            ("request-changes under a list", payload(), TIERS),
        ] {
            let (_tmp, launch) = launch_under(config);
            let body = value.to_string();
            let draft = launch
                .submit(&body)
                .unwrap_or_else(|e| panic!("{name}: {e:#}"));
            // The accepted bytes are the submitted bytes, and the parent reads them.
            assert_eq!(
                std::fs::read_to_string(&launch.path).unwrap(),
                body,
                "{name}"
            );
            assert!(!launch.pending_path().exists(), "{name}");
            assert_eq!(launch.read().unwrap().findings, draft.findings, "{name}");
            // What the submission accepts, the recorder's own decision accepts.
            let decided = crate::gate::kernel_deps::review_blocking::apply_policy(
                &policy(config),
                &draft.record_findings(),
                &[],
            );
            assert_eq!(
                rv::incoming_disposition(&draft.verdict, &decided),
                rv::VerdictKind::parse(&draft.verdict),
                "{name}"
            );
        }
    }
    /// A list that cannot be honoured blocks every finding, deferrals
    /// included, and the message offers request-changes only.
    // trace:BUG-2707 | ai:claude
    #[test]
    fn bug_2707_unreadable_policy_refuses_even_a_deferred_approval() {
        let config = "[review]\nblocking_tiers = [\"nonsense\"]\n";
        assert!(policy(config).fails_closed());
        let (_tmp, launch) = launch_under(config);
        let deferred = serde_json::json!({"disposition":"deferred","rationale":"later is fine"});
        let err = refusal(
            &launch,
            &approved(serde_json::json!([finding("p3", deferred)])),
        );
        assert!(err.contains("none can be deferred"), "{err}");
        assert!(!err.contains("(b)"), "{err}");
        launch
            .submit(&approved(serde_json::json!([])).to_string())
            .unwrap();
    }
    /// The policy is the launching project's, recorded in the launch: the
    /// same draft is decided the same way whichever checkout or directory
    /// the reviewer submits from, and a launch record that carries no
    /// readable policy fails closed.
    // trace:BUG-2707 | ai:claude
    #[test]
    fn bug_2707_submission_is_decided_by_the_parent_launch_policy() {
        use crate::gate::kernel_deps::review_blocking as rb;
        let deferred = serde_json::json!({"disposition":"deferred","rationale":"later is fine"});
        let draft = approved(serde_json::json!([finding("p2", deferred)]));
        // A checkout without the list (a private review clone, another
        // project) would pass this approval under its own policy.
        let elsewhere = policy("");
        assert_eq!(
            rv::approval_refusal(
                &elsewhere,
                "approved",
                &[draft["findings"][0].as_str().unwrap().to_string()]
            ),
            None
        );
        // The parent blocks p2: the submission refuses, with no reference to
        // the directory it runs from.
        let (_tmp, strict) = launch_under(TIERS);
        let err = refusal(&strict, &draft);
        assert!(err.contains("in blocking class p2"), "{err}");
        // The parent has no list: the same draft is accepted.
        let (_tmp, lenient) = launch_under("");
        lenient.submit(&draft.to_string()).unwrap();
        // The recorded policy round-trips every shape.
        for config in ["", TIERS, "[review]\nblocking_tiers = [\"nonsense\"]\n"] {
            let effective = policy(config);
            let restored = rb::from_snapshot(&rb::to_snapshot(&effective));
            assert_eq!(restored.policy, effective.policy, "{config}");
            assert_eq!(
                restored.fails_closed(),
                effective.fails_closed(),
                "{config}"
            );
        }
        // A launch record whose policy is missing or unrecognised fails closed.
        for policy_value in [
            serde_json::Value::Null,
            serde_json::json!({"blocking": "tiers", "tiers": ["typo"]}),
            serde_json::json!({"blocking": "tiers"}),
            serde_json::json!({"blocking": "later"}),
        ] {
            assert!(
                rb::from_snapshot(&policy_value).fails_closed(),
                "{policy_value}"
            );
            let (_tmp, launch) = launch_under("");
            let record = launch.path.with_file_name(LAUNCH_FILE);
            let mut value: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
            value["policy"] = policy_value.clone();
            std::fs::write(&record, value.to_string()).unwrap();
            let err = refusal(&launch, &draft);
            assert!(
                err.contains("none can be deferred"),
                "{policy_value}: {err}"
            );
        }
    }
    /// The submission applies the launcher's schema and launch binding, and
    /// each refusal leaves nothing accepted.
    // trace:BUG-2707 | ai:claude
    #[test]
    fn bug_2707_submission_applies_the_schema_and_launch_binding() {
        let (tmp, launch) = launch_under("");
        let mut wrong_pr = payload();
        wrong_pr["pr"] = 60.into();
        let err = refusal(&launch, &wrong_pr);
        assert!(err.contains("set \"pr\" to 59"), "{err}");
        let mut wrong_sha = payload();
        wrong_sha["reviewed_sha"] = "ffffffffffffffffffffffffffffffffffffffff".into();
        let err = refusal(&launch, &wrong_sha);
        assert!(
            err.contains(&format!("this launch reviews {HEAD}")),
            "{err}"
        );
        for (field, value) in [
            ("version", serde_json::json!(2)),
            ("reviewed_sha", serde_json::json!("0123456")),
            ("verdict", serde_json::json!("approved if green")),
            ("summary", serde_json::json!(" ")),
            ("recorded_by", serde_json::json!("forged")),
        ] {
            let mut data = payload();
            data[field] = value;
            assert!(launch.submit(&data.to_string()).is_err(), "{field}");
        }
        let mut waiting = payload();
        waiting["verdict"] = "waiting".into();
        assert!(launch.submit(&waiting.to_string()).is_err());
        assert!(!launch.path.exists());
        std::fs::remove_file(launch.pending_path()).unwrap();
        let err = format!("{:#}", submit_draft(launch.launch_dir()).unwrap_err());
        assert!(err.contains("no draft to submit at"), "{err}");
        assert!(err.contains("pending.json"), "{err}");
        // Not a launch directory.
        let err = format!("{:#}", submit_draft(tmp.path()).unwrap_err());
        assert!(err.contains("is not a review launch directory"), "{err}");
        let err = format!(
            "{:#}",
            submit_draft(&tmp.path().join("absent")).unwrap_err()
        );
        assert!(err.contains("no review launch directory"), "{err}");
    }
    /// The pending draft is opened once, without following links or waiting
    /// on a FIFO, and the size and file-type checks are made on that handle.
    // trace:BUG-2707 | ai:claude
    #[test]
    fn bug_2707_submission_reads_one_bounded_no_follow_handle() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let (done, wait) = std::sync::mpsc::channel::<()>();
        let watchdog = std::thread::spawn(move || {
            if matches!(
                wait.recv_timeout(std::time::Duration::from_secs(30)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ) {
                eprintln!("review draft submission FIFO test exceeded 30 seconds");
                std::process::exit(124);
            }
        });
        let (tmp, launch) = launch_under("");
        let pending = launch.pending_path();
        let submit = || submit_draft(launch.launch_dir());
        let other = tmp.path().join("other.json");
        std::fs::write(&other, payload().to_string()).unwrap();
        std::os::unix::fs::symlink(&other, &pending).unwrap();
        assert!(submit().is_err(), "symlink");
        std::fs::remove_file(&pending).unwrap();
        std::fs::hard_link(&other, &pending).unwrap();
        assert!(submit().is_err(), "hard link");
        std::fs::remove_file(&pending).unwrap();
        let name = std::ffi::CString::new(pending.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        // O_NONBLOCK must refuse without waiting for a FIFO writer.
        assert!(submit().is_err(), "fifo");
        std::fs::remove_file(&pending).unwrap();
        File::create(&pending).unwrap().set_len(LIMIT + 1).unwrap();
        assert!(submit().is_err(), "oversize");
        std::fs::remove_file(&pending).unwrap();
        assert!(!launch.path.exists());
        std::fs::write(&pending, payload().to_string()).unwrap();
        submit().unwrap();
        drop(done);
        watchdog.join().unwrap();
    }
    /// The prompt's command survives a shell when the review tree's path
    /// holds whitespace and a quote, and every handoff instruction makes
    /// acceptance the condition for exiting.
    // trace:BUG-2707 | ai:claude
    #[test]
    fn bug_2707_prompt_command_is_shell_quoted_and_gates_exit_on_acceptance() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = tmp.path().join("re view's $(tree) `x`");
        std::fs::create_dir_all(&tree).unwrap();
        let launch = LaunchDraft::prepare(&tree, 59, HEAD.into(), &policy("")).unwrap();
        let rendered = launch.prompt("fresh review");
        let command = launch.submit_command();
        assert!(rendered.contains(&command), "{rendered}");
        let argv = command
            .strip_prefix("aida review submit-draft ")
            .expect("the command names the subcommand first");
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("printf '%s\\n' {argv}")])
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8(out.stdout)
                .unwrap()
                .trim_end_matches('\n'),
            launch.launch_dir().to_str().unwrap(),
            "one argument, uninterpreted"
        );
        // The same string, parsed by a shell, submits this launch's draft.
        std::fs::write(launch.pending_path(), payload().to_string()).unwrap();
        let out = std::process::Command::new("/bin/sh")
            .args([
                "-c",
                &format!("for a in {argv}; do printf '%s\\n' \"$a\"; done | wc -l"),
            ])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "1");
        submit_draft(launch.launch_dir()).unwrap();
        assert!(launch.read().is_ok());

        assert!(rendered.contains("Any unresolved finding means request-changes"));
        assert!(rendered.contains("Exit only after the submission command has accepted the draft"));
        assert!(
            !rendered.contains("Once the draft is written"),
            "{rendered}"
        );
        assert!(!rendered.contains("check-draft"), "{rendered}");
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../aida-core/templates");
        for doc in ["commands/aida-review.md", "skills/aida-review.md"] {
            let text = std::fs::read_to_string(root.join(doc)).unwrap();
            let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(flat.contains("aida review submit-draft"), "{doc}");
            assert!(
                flat.contains("Exit only after the submission is accepted"),
                "{doc}"
            );
            assert!(!flat.contains("there and exit"), "{doc}");
            assert!(!flat.contains("check-draft"), "{doc}");
        }
    }
    #[test]
    fn standalone_review_verdict_draft_rejects_wrong_missing_and_authority_fields() {
        let (_tmp, launch) = fixture();
        assert!(LaunchDraft::prepare_local(_tmp.path(), 59, String::new()).is_err());
        for field in [
            "reviewer_draft",
            "recorded_by",
            "recorder_attestation",
            "release_grant",
            "covered_specs",
            "ledger",
            "command",
        ] {
            let mut value = payload();
            value[field] = serde_json::json!({"grant_id":"forged"});
            write(&launch, &value);
            assert!(launch.read().is_err(), "{field}");
        }
        for (field, value) in [
            ("version", serde_json::json!(2)),
            ("pr", serde_json::json!(60)),
            ("reviewed_sha", serde_json::json!("0123456")),
            (
                "reviewed_sha",
                serde_json::json!("ffffffffffffffffffffffffffffffffffffffff"),
            ),
            ("verdict", serde_json::json!("approved if green")),
            ("summary", serde_json::json!("")),
        ] {
            let mut data = payload();
            data[field] = value;
            write(&launch, &data);
            assert!(launch.read().is_err(), "{field}");
        }
        for field in ["reviewed_sha", "findings", "summary", "pr"] {
            let mut data = payload();
            data.as_object_mut().unwrap().remove(field);
            write(&launch, &data);
            assert!(launch.read().is_err(), "{field}");
        }
        launch.place("{\"version\":1,\"version\":1}");
        assert!(launch.read().is_err());
    }
    #[test]
    fn standalone_review_verdict_draft_rejects_links_fifo_oversize_and_malformed() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let (done, wait) = std::sync::mpsc::channel::<()>();
        let watchdog = std::thread::spawn(move || {
            if matches!(
                wait.recv_timeout(std::time::Duration::from_secs(30)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ) {
                eprintln!("review draft FIFO test exceeded 30 seconds");
                std::process::exit(124);
            }
        });
        use std::os::unix::fs::symlink;
        let (tmp, launch) = fixture();
        let other = tmp.path().join("other.json");
        std::fs::write(&other, serde_json::to_vec(&payload()).unwrap()).unwrap();
        symlink(&other, &launch.path).unwrap();
        assert!(launch.read().is_err());
        std::fs::remove_file(&launch.path).unwrap();
        std::fs::hard_link(&other, &launch.path).unwrap();
        assert!(launch.read().is_err());
        std::fs::remove_file(&launch.path).unwrap();
        let name = std::ffi::CString::new(launch.path.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        // O_NONBLOCK must refuse without waiting for a FIFO writer.
        assert!(launch.read().is_err());
        std::fs::remove_file(&launch.path).unwrap();
        let file = File::create(&launch.path).unwrap();
        file.set_len(LIMIT + 1).unwrap();
        assert!(launch.read().is_err());
        launch.place("{broken");
        assert!(launch.read().is_err());
        drop(done);
        watchdog.join().unwrap();
    }
    #[test]
    fn standalone_review_verdict_draft_pins_directory_and_excludes_old_launches() {
        let (tmp, first) = fixture();
        write(&first, &payload());
        let second = LaunchDraft::prepare_local(tmp.path(), 59, HEAD.into()).unwrap();
        assert!(first.validate_current_head(Some(HEAD)).is_ok());
        assert!(first.validate_current_head(None).is_err());
        assert!(first
            .validate_current_head(Some("ffffffffffffffffffffffffffffffffffffffff"))
            .is_err());
        assert_ne!(first.path, second.path);
        assert!(second.read().is_err());
        let dir = first.path.parent().unwrap();
        let moved = dir.with_extension("moved");
        std::fs::rename(dir, &moved).unwrap();
        std::fs::create_dir(dir).unwrap();
        let mut forged = payload();
        forged["summary"] = serde_json::json!("replacement");
        write(&first, &forged);
        assert_eq!(first.read().unwrap().summary, "complete rationale");
        let (_tmp, mut stale) = fixture();
        write(&stale, &payload());
        stale.since = SystemTime::now() + std::time::Duration::from_secs(60);
        assert!(stale.read().is_err());
    }
}

#[cfg(test)]
mod standalone_review_verdict_parent_tests {
    use super::*;
    use aida_core::DatabaseBackend;

    struct Fixture {
        tmp: tempfile::TempDir,
        _env: crate::test_env::EnvVarsGuard,
        root: PathBuf,
        head: String,
        parent: String,
        child: String,
        since: SystemTime,
    }
    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().join("project");
            let home = tmp.path().join("home");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::create_dir_all(&home).unwrap();
            let mut env = crate::test_env::EnvVarsGuard::apply(&[
                ("HOME", Some(home.to_str().unwrap())),
                ("AIDA_HOME", Some(home.to_str().unwrap())),
                ("AIDA_PROJECT_ROOT", Some(root.to_str().unwrap())),
                ("AIDA_DRIVE_ROOT", Some(root.to_str().unwrap())),
                ("AIDA_REVIEW_VERDICT_FILE", None),
                ("AIDA_SESSION_GRANT", None),
                ("AIDA_CONTAINED", None),
                ("AIDA_STANDALONE_IMPLEMENTER", None),
                ("AIDA_SESSION_ID", None),
                ("AIDA_AGENT_TYPE", Some("codex")),
                ("AIDA_AGENT_NAME", Some("stub-reviewer")),
            ]);
            let head = crate::test_git::commit_fixture(&root);
            let parent = sa::test_support::mint_grant_for(
                &root,
                &crate::current_user_id(None),
                "advisor",
                &["reviewer"],
            );
            env.set_key(sa::GRANT_ENV, &parent);
            let since = SystemTime::now();
            let child = crate::launch::issue_launch_child(
                &root,
                "reviewer",
                &crate::current_user_id(None),
                false,
            )
            .unwrap()
            .id;
            let store = crate::detect_distributed_store_from(&root).unwrap();
            let backend = crate::test_env::home_fixture_cached_backend(
                &store,
                &aida_core::CachedGitBackend::default_cache_path(&store),
            )
            .unwrap();
            let mut req = aida_core::Requirement::new("reviewed change".into(), "fixture".into());
            req.spec_id = Some("STORY-77".into());
            backend.add_requirement(req).unwrap();
            Self {
                tmp,
                _env: env,
                root,
                head,
                parent,
                child,
                since,
            }
        }
        fn reviewer(&self) -> Result<VerifiedReviewer> {
            VerifiedReviewer::from_launch(
                &self.root,
                &self.child,
                &self.parent,
                self.since,
                "independent-review-session",
            )
        }
        fn record(
            &self,
            verdict: &str,
            findings: &[String],
            reviewer: Option<&VerifiedReviewer>,
        ) -> Result<()> {
            crate::with_review_pr_subject(
                crate::ReviewPrSubject {
                    head_ref: "feature".into(),
                    commit_subjects: vec!["change (STORY-77)".into()],
                },
                || {
                    crate::handle_review_record_context_at(
                        self.root.clone(),
                        "STORY-77",
                        verdict,
                        Some(&self.head),
                        Some("feature"),
                        Some("complete child rationale"),
                        findings,
                        &[],
                        Some(59),
                        false,
                        |_, _| Ok(self.head.clone()),
                        reviewer,
                    )
                },
            )
        }
    }
    #[cfg(unix)]
    #[test]
    fn standalone_review_verdict_parent_records_after_unbound_child_refusal_and_retries() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let launch = LaunchDraft::prepare_local(&f.root, 59, f.head.clone()).unwrap();
        let refused = ra::with_identity_source(ra::IdentitySource::default(), || {
            f.record("approved", &[], None)
        })
        .unwrap_err();
        assert!(
            format!("{refused:#}").contains("no identity AIDA can bind"),
            "{refused:#}"
        );
        // An external child needs only write access to its data file, no grant
        // selector, identity variables, model logs or command replay.
        let body = serde_json::json!({"version":1,"pr":59,"reviewed_sha":f.head,"verdict":"approved","summary":"complete child rationale","findings":[]});
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "cat > \"$1\"", "stub"])
            .arg(&launch.path)
            .env_clear()
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(body.to_string().as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
        launch.stamp();
        let draft = launch.read().unwrap();
        let reviewer = f.reviewer().unwrap().with_draft(&launch, &draft).unwrap();
        f.record(&draft.verdict, &draft.findings, Some(&reviewer))
            .unwrap();
        let checks = crate::queue_cmd::StandaloneVerdictChecks {
            project_root: &f.root,
            pr: 59,
            head_sha: Some(f.head.clone()),
            launcher_grant: Some(f.parent.clone()),
            child_grant: Some(f.child.clone()),
        };
        let sidecar = f.root.join(".aida/review-verdicts/STORY-77.json");
        std::fs::write(&sidecar, serde_json::json!({"verdict":"approved","reviewed_sha":f.head,"summary":"unstamped sidecar"}).to_string()).unwrap();
        let collected = rv::verify_recorded_draft(
            &f.root,
            &rv::verdict_path(&f.root, "PR-59"),
            &["STORY-77".into()],
            f.since,
            &checks,
        )
        .unwrap();
        assert_eq!(
            collected.verdict.summary.as_deref(),
            Some("complete child rationale")
        );
        std::fs::remove_file(sidecar).unwrap();
        let recorded = rv::read_recorded_verdict(&f.root, "PR-59").unwrap();
        assert_eq!(recorded.reviewed_sha.as_deref(), Some(f.head.as_str()));
        assert_eq!(
            recorded.summary.as_deref(),
            Some("complete child rationale")
        );
        let att = recorded.attestation.unwrap();
        assert_eq!(att.grant_id.as_deref(), Some(f.child.as_str()));
        assert!(att
            .identity
            .contains(&"session:independent-review-session".into()));
        let before = crate::gate::ledger::project::verdict_records(&f.root, "PR-59").len();
        f.record(&draft.verdict, &draft.findings, Some(&reviewer))
            .unwrap();
        assert_eq!(
            crate::gate::ledger::project::verdict_records(&f.root, "PR-59").len(),
            before
        );
        assert!(
            launch.path.exists(),
            "draft retained for retry until durable collection"
        );
        let mut changed = launch.read().unwrap();
        changed.summary.push_str(" changed");
        let changed_context = f.reviewer().unwrap().with_draft(&launch, &changed).unwrap();
        assert!(
            f.record(&changed.verdict, &changed.findings, Some(&changed_context))
                .is_err(),
            "changed replay must refuse"
        );
        f.record(
            "request-changes",
            &["later review defect".into()],
            Some(&f.reviewer().unwrap()),
        )
        .unwrap();
        assert!(
            f.record(&draft.verdict, &draft.findings, Some(&reviewer))
                .is_err(),
            "older replay must not displace a later review"
        );

        let _ = &f.tmp;
    }
    fn checks(f: &Fixture) -> crate::queue_cmd::StandaloneVerdictChecks<'_> {
        crate::queue_cmd::StandaloneVerdictChecks {
            project_root: &f.root,
            pr: 59,
            head_sha: Some(f.head.clone()),
            launcher_grant: Some(f.parent.clone()),
            child_grant: Some(f.child.clone()),
        }
    }
    fn deferred(n: u32) -> String {
        serde_json::json!({"text": format!("P3 follow-up {n}"), "severity": "minor",
            "disposition": "deferred", "rationale": "advisor deferred to a follow-up spec"})
        .to_string()
    }
    fn approved_with_deferred(f: &Fixture, launch: &LaunchDraft) -> ReviewerDraft {
        let body = serde_json::json!({"version":1,"pr":59,"reviewed_sha":f.head,"verdict":"approved",
            "summary":"complete child rationale","findings":[deferred(1), deferred(2)]});
        launch.place(&body.to_string());
        launch.read().unwrap()
    }
    /// The p18 shape (PR #4218, 2026-11-17): APPROVED with two deferred P3s
    /// whose delivery failed after the barrier was written. Retrying the same
    /// handoff completes the approval at the head; it never re-records the
    /// barrier.
    // trace:BUG-2564 | ai:claude
    #[test]
    fn standalone_review_verdict_parent_completes_deferred_approval_after_failed_delivery() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let launch = LaunchDraft::prepare_local(&f.root, 59, f.head.clone()).unwrap();
        let draft = approved_with_deferred(&f, &launch);
        let reviewer = f.reviewer().unwrap().with_draft(&launch, &draft).unwrap();
        crate::gate::ledger::FAIL_NEXT_FINDING_DELIVERY.with(|c| c.set(true));
        assert!(f
            .record(&draft.verdict, &draft.findings, Some(&reviewer))
            .is_err());
        let pending = rv::read_recorded_verdict(&f.root, "PR-59").unwrap();
        assert_eq!(
            pending.kind,
            rv::VerdictKind::RequestChanges,
            "barrier stands"
        );
        // The launch's own barrier is replay evidence, not a legacy conflict.
        rv::check_draft_legacy_conflicts(
            &f.root,
            &f.root,
            &rv::verdict_path(&f.root, "PR-59"),
            &["STORY-77".into()],
            f.since,
            &draft.facts(),
            reviewer.own_handoff().as_ref(),
            &checks(&f),
        )
        .unwrap();
        f.record(&draft.verdict, &draft.findings, Some(&reviewer))
            .unwrap();
        let path = rv::verdict_path(&f.root, "PR-59");
        let current = rv::read_current_verdict_object(&path).unwrap();
        assert_ne!(current.get("sweep_pending"), Some(&serde_json::json!(true)));
        let recorded = rv::read_recorded_verdict(&f.root, "PR-59").unwrap();
        assert_eq!(recorded.kind, rv::VerdictKind::Approved);
        assert_eq!(recorded.reviewed_sha.as_deref(), Some(f.head.as_str()));
        assert_eq!(recorded.findings, ["P3 follow-up 1", "P3 follow-up 2"]);
        rv::verify_recorded_draft(&f.root, &path, &["STORY-77".into()], f.since, &checks(&f))
            .unwrap();
        // A completed handoff replays as a no-op.
        let before = crate::gate::ledger::project::verdict_records(&f.root, "PR-59").len();
        f.record(&draft.verdict, &draft.findings, Some(&reviewer))
            .unwrap();
        assert_eq!(
            crate::gate::ledger::project::verdict_records(&f.root, "PR-59").len(),
            before
        );
    }
    /// One contract on both sides: the approved draft with plain p3 and style
    /// findings that the submission refuses is the draft the recorder
    /// refuses (nothing recorded), and the deferred approval the submission
    /// accepts is recorded as an approval.
    // trace:BUG-2707 | ai:claude
    #[test]
    fn bug_2707_submission_and_recorder_agree_on_approved_with_findings() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let launch = LaunchDraft::prepare_local(&f.root, 59, f.head.clone()).unwrap();
        let body = serde_json::json!({"version":1,"pr":59,"reviewed_sha":f.head,"verdict":"approved",
            "summary":"complete child rationale",
            "findings":["P3: rename the helper", "style: wrap the long line"]});
        let why = format!("{:#}", launch.submit(&body.to_string()).unwrap_err());
        assert!(why.contains("2 unresolved findings"), "{why}");
        assert!(
            launch.read().is_err(),
            "a refused draft is not a submission"
        );
        // Had that draft reached the recorder, it would refuse it too.
        launch.place(&body.to_string());
        let draft = launch.read().unwrap();
        let reviewer = f.reviewer().unwrap().with_draft(&launch, &draft).unwrap();
        let refused = f
            .record(&draft.verdict, &draft.record_findings(), Some(&reviewer))
            .unwrap_err();
        assert!(
            format!("{refused:#}").contains(rv::EXPLICIT_APPROVAL_REFUSAL),
            "{refused:#}"
        );
        assert!(rv::read_recorded_verdict(&f.root, "PR-59").is_none());

        let launch = LaunchDraft::prepare_local(&f.root, 59, f.head.clone()).unwrap();
        let body = serde_json::json!({"version":1,"pr":59,"reviewed_sha":f.head,"verdict":"approved",
            "summary":"complete child rationale","findings":[deferred(1), deferred(2)]});
        launch.submit(&body.to_string()).unwrap();
        let draft = launch.read().unwrap();
        let reviewer = f.reviewer().unwrap().with_draft(&launch, &draft).unwrap();
        f.record(&draft.verdict, &draft.record_findings(), Some(&reviewer))
            .unwrap();
        let recorded = rv::read_recorded_verdict(&f.root, "PR-59").unwrap();
        assert_eq!(recorded.kind, rv::VerdictKind::Approved);
        assert_eq!(recorded.findings, ["P3 follow-up 1", "P3 follow-up 2"]);
    }
    /// A completed request-changes draft whose p2 finding the `["p1"]` list
    /// deferred was recorded as an approval. Replaying that same launch after
    /// the list became invalid, unreadable or newly blocking re-decides the
    /// stored finding under the current policy: the record (and its routing)
    /// is request-changes, never the stale approval.
    // trace:STORY-2913 | ai:claude
    fn completed_replay_is_redecided(case: usize) {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        use crate::review_blocking::tests as policy;
        {
            let f = Fixture::new();
            // Layer the list onto the fixture's own configuration, which the
            // reviewer grant checks also read.
            let config = f.root.join(".aida/config.toml");
            let original = std::fs::read_to_string(&config).unwrap_or_default();
            let set = |tiers: &str| {
                std::fs::create_dir_all(config.parent().unwrap()).unwrap();
                std::fs::write(
                    &config,
                    format!("{original}\n[review]\nblocking_tiers = {tiers}\n"),
                )
                .unwrap();
                crate::review_blocking::commit_config_for_test(&f.root);
            };
            let (label, change): (&str, Box<dyn Fn()>) = match case {
                0 => ("invalid", Box::new(|| set("[\"p1\", \"typo\"]"))),
                1 => (
                    "unavailable",
                    Box::new(|| policy::publish_unreadable_config(&f.root)),
                ),
                _ => ("newly-blocking", Box::new(|| set("[\"p1\", \"p2\"]"))),
            };
            set("[\"p1\"]");
            let launch = LaunchDraft::prepare_local(&f.root, 59, f.head.clone()).unwrap();
            let body = serde_json::json!({"version":1,"pr":59,"reviewed_sha":f.head,
                "verdict":"request-changes","summary":"complete child rationale",
                "findings":[policy::p2_finding()]});
            launch.place(&body.to_string());
            let draft = launch.read().unwrap();
            let reviewer = f.reviewer().unwrap().with_draft(&launch, &draft).unwrap();
            f.record(&draft.verdict, &draft.findings, Some(&reviewer))
                .unwrap();
            let first = rv::read_recorded_verdict(&f.root, "PR-59").unwrap();
            assert_eq!(first.kind, rv::VerdictKind::Approved, "{label}");

            change();
            let replayed = f.record(&draft.verdict, &draft.findings, Some(&reviewer));
            let path = rv::verdict_path(&f.root, "PR-59");
            let current = rv::read_current_verdict_object(&path).unwrap();
            match replayed {
                Ok(()) => {
                    assert_eq!(current["verdict"], "request-changes", "{label}");
                    assert_eq!(
                        current["finding_dispositions"][0]["disposition"], "blocking",
                        "{label}"
                    );
                    assert!(
                        crate::gate::merge_hold_core::read_hold(&f.root, 59).is_some(),
                        "{label}: routing and the record agree"
                    );
                }
                Err(err) => panic!("{label}: replay must re-decide, not fail: {err:#}"),
            }
            assert!(
                !rv::read_recorded_verdict(&f.root, "PR-59")
                    .unwrap()
                    .kind
                    .approves(),
                "{label}"
            );
        }
    }
    #[test]
    fn story_2913_completed_replay_is_redecided_under_an_invalid_policy() {
        completed_replay_is_redecided(0);
    }
    #[test]
    fn story_2913_completed_replay_is_redecided_under_an_unavailable_policy() {
        completed_replay_is_redecided(1);
    }
    #[test]
    fn story_2913_completed_replay_is_redecided_under_a_newly_blocking_policy() {
        completed_replay_is_redecided(2);
    }
    /// The unit boundary: a record whose re-decided disposition is not the
    /// routed kind is refused, never rewritten.
    // trace:STORY-2913 | ai:claude
    #[test]
    fn story_2913_replay_whose_redecision_differs_from_routing_refuses() {
        use crate::review_blocking::tests as policy;
        let tmp = tempfile::tempdir().unwrap();
        policy::set_policy(tmp.path(), "[\"p1\", \"p2\"]");
        let now = crate::gate::kernel_deps::review_blocking::effective(tmp.path());
        let stale = serde_json::json!({"verdict":"approved","findings":["contained defect"],
            "finding_dispositions":[{"severity":"minor","blocking_tier":"p2",
                "disposition":"deferred","rationale":"later"}]});
        let err = rv::redecide_replay_record(
            stale.as_object().unwrap().clone(),
            &now,
            rv::VerdictKind::Approved,
        )
        .unwrap_err();
        assert!(err.contains("nothing was recorded"), "{err}");
    }
    /// C2: a record from another launch conflicts even when it was made by
    /// the same reviewer seat.
    // trace:BUG-2564 | ai:claude
    #[test]
    fn standalone_review_verdict_parent_other_launch_same_seat_still_conflicts() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let first = LaunchDraft::prepare_local(&f.root, 59, f.head.clone()).unwrap();
        let draft = approved_with_deferred(&f, &first);
        let own = f.reviewer().unwrap().with_draft(&first, &draft).unwrap();
        let other = LaunchDraft::prepare_local(&f.root, 59, f.head.clone()).unwrap();
        other.place(
            &serde_json::json!({"version":1,"pr":59,"reviewed_sha":f.head,"verdict":"request-changes",
                "summary":"another launch","findings":["blocking defect"]})
            .to_string(),
        );
        let other_draft = other.read().unwrap();
        let other_reviewer = f
            .reviewer()
            .unwrap()
            .with_draft(&other, &other_draft)
            .unwrap();
        f.record(
            &other_draft.verdict,
            &other_draft.findings,
            Some(&other_reviewer),
        )
        .unwrap();
        let err = rv::check_draft_legacy_conflicts(
            &f.root,
            &f.root,
            &rv::verdict_path(&f.root, "PR-59"),
            &["STORY-77".into()],
            f.since,
            &draft.facts(),
            own.own_handoff().as_ref(),
            &checks(&f),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("conflicts"), "{err:#}");
    }
    #[test]
    fn standalone_review_verdict_parent_refusal_preserves_findings_and_hold() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let reviewer = f.reviewer().unwrap();
        let findings = vec!["concrete blocking defect".into()];
        f.record("request-changes", &findings, Some(&reviewer))
            .unwrap();
        let recorded = rv::read_recorded_verdict(&f.root, "PR-59").unwrap();
        assert_eq!(recorded.findings, findings);
        assert!(crate::gate::merge_hold_core::read_hold(&f.root, 59).is_some());
    }
    #[test]
    fn standalone_review_verdict_parent_rejects_bad_grants_and_launcher_identity() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let original = sa::read_grant(&f.child).unwrap();
        for edit in 0..7 {
            sa::test_support::rewrite_grant(&f.child, |g| match edit {
                0 => g.seat = "implementer".into(),
                1 => g.expires_at = chrono::Utc::now() - chrono::Duration::seconds(1),
                2 => g.revoked_at = Some(chrono::Utc::now()),
                3 => g.subject = "another-agent".into(),
                4 => g.binding.clear(),
                5 => g.session_id.clear(),
                _ => g.parent_grant_id = None,
            });
            assert!(f.reviewer().is_err(), "case {edit}");
            sa::test_support::put_grant(&original);
        }
        assert!(
            VerifiedReviewer::from_launch(&f.root, &f.parent, &f.parent, f.since, "session")
                .is_err()
        );
        assert!(VerifiedReviewer::from_launch(
            &f.root,
            &f.child,
            &f.parent,
            SystemTime::now() + std::time::Duration::from_secs(1),
            "session"
        )
        .is_err());
        ra::with_identity_source(
            ra::IdentitySource {
                session_ids: vec!["independent-review-session".into()],
                ..Default::default()
            },
            || assert!(f.reviewer().is_err()),
        );
    }
    #[test]
    fn standalone_review_verdict_parent_rejects_resumed_implementer_identity() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let reviewer = f.reviewer().unwrap();
        let author = serde_json::json!({"scope":"STORY-77","lease_id":"implemented-session",
            "branch":"feature","tokens":["session:independent-review-session"],
            "recorded_at":chrono::Utc::now().to_rfc3339()});
        std::fs::write(
            f.root.join(".aida/review-authorship.jsonl"),
            format!("{author}\n"),
        )
        .unwrap();
        let err = f.record("approved", &[], Some(&reviewer)).unwrap_err();
        assert!(
            format!("{err:#}").contains("cannot approve its own work"),
            "{err:#}"
        );
        assert!(rv::read_recorded_verdict(&f.root, "PR-59").is_none());
    }

    #[test]
    fn standalone_review_verdict_parent_rejects_conflicting_and_malformed_legacy_sources() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        struct Checks;
        impl rv::VerdictCollectionChecks for Checks {
            fn check_record(&self, _: &rv::VerdictCandidate) -> std::result::Result<(), String> {
                Ok(())
            }
            fn check_identity(&self, id: &str) -> std::result::Result<(), String> {
                if id == "child" {
                    Ok(())
                } else {
                    Err("wrong reviewer grant".into())
                }
            }
        }
        let dir = f.root.join(".aida/review-verdicts");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("STORY-77.json");
        let draft = ReviewerDraft {
            version: 1,
            pr: 59,
            reviewed_sha: f.head.clone(),
            verdict: "approved".into(),
            summary: "ok".into(),
            findings: vec![],
        };
        let check = || {
            rv::check_draft_legacy_conflicts(
                &f.root,
                &f.root,
                &dir.join("PR-59.json"),
                &["STORY-77".into()],
                f.since,
                &draft.facts(),
                None,
                &Checks,
            )
        };
        for value in [
            serde_json::json!({"verdict":"request-changes","reviewed_sha":f.head,"findings":["defect"]}),
            serde_json::json!({"verdict":"approved","reviewed_sha":f.head,"recorder_attestation":{"grant_id":"launcher"}}),
        ] {
            std::fs::write(&path, value.to_string()).unwrap();
            assert!(check().is_err());
        }
        std::fs::write(&path, "{broken").unwrap();
        assert!(check().is_err());
        std::fs::write(&path,serde_json::json!({"verdict":"approved","reviewed_sha":f.head,"recorder_attestation":{"grant_id":"child"}}).to_string()).unwrap();
        assert!(check().is_ok());
        assert!(rv::read_recorded_verdict(&f.root, "PR-59").is_none());
    }
    #[test]
    fn standalone_review_verdict_parent_retries_record_and_publication_failure_without_rounds() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let launch = LaunchDraft::prepare_local(&f.root, 59, f.head.clone()).unwrap();
        let data = serde_json::json!({"version":1,"pr":59,"reviewed_sha":f.head,"verdict":"approved","summary":"complete child rationale","findings":[]});
        launch.place(&data.to_string());
        let reviewer = f
            .reviewer()
            .unwrap()
            .with_draft(&launch, &launch.read().unwrap())
            .unwrap();
        let ledger = f.root.join(".aida/gate-ledger");
        std::fs::write(&ledger, "blocked ledger directory").unwrap();
        assert!(f.record("approved", &[], Some(&reviewer)).is_err());
        assert!(
            launch.read().is_ok(),
            "failed recording retains the recoverable draft"
        );
        std::fs::remove_file(&ledger).unwrap();
        let publication = f.root.join(".aida/review-publisher.json");
        std::fs::write(&publication, "{invalid publisher configuration").unwrap();
        f.record("approved", &[], Some(&reviewer)).unwrap();
        let before = crate::gate::ledger::project::verdict_records(&f.root, "PR-59").len();
        assert!(rv::read_recorded_verdict(&f.root, "PR-59")
            .unwrap()
            .kind
            .approves());
        // Canonical evidence survives publication failure; retrying after repair
        // uses the same launch identity and does not append another round.
        std::fs::remove_file(publication).unwrap();
        f.record("approved", &[], Some(&reviewer)).unwrap();
        assert_eq!(
            crate::gate::ledger::project::verdict_records(&f.root, "PR-59").len(),
            before
        );
        launch.consumed().unwrap();
        assert!(!launch.path.exists());
        assert!(launch.path.with_file_name("recorded.json").exists());
    }
    #[test]
    fn standalone_review_verdict_parent_waiting_draft_clears_findings_and_retains_hold() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let reviewer = f.reviewer().unwrap();
        f.record("request-changes", &["fixed defect".into()], Some(&reviewer))
            .unwrap();
        let draft = ReviewerDraft {
            version: 1,
            pr: 59,
            reviewed_sha: f.head.clone(),
            verdict: "waiting".into(),
            summary: "operator must supply canary before a fresh review".into(),
            findings: vec![],
        };
        assert!(draft.record_findings().is_empty());
        f.record(&draft.verdict, &draft.record_findings(), Some(&reviewer))
            .unwrap();
        let v = rv::read_recorded_verdict(&f.root, "PR-59").unwrap();
        assert_eq!(v.kind, rv::VerdictKind::Waiting);
        assert!(v.findings.is_empty());
        assert!(crate::gate::merge_hold_core::read_hold(&f.root, 59).is_some());
    }
}
