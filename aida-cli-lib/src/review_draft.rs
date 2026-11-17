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
    pub(crate) fn prepare(worktree: &Path, pr: u64, head: String) -> Result<Self> {
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
        // Use the same filesystem clock as the future draft, rather than
        // wall time: cached kernel timestamps can lag a just-read clock.
        let since = dir.metadata()?.modified()?;
        Ok(Self {
            dir,
            path: root.join(name).join("draft.json"),
            pr,
            head,
            since,
        })
    }

    pub(crate) fn prompt(&self, prompt: &str) -> String {
        format!("{prompt}\n\nStandalone reviewer handoff (supersedes in-sandbox recording): write UTF-8 JSON data to the absolute path `{}` before exiting, on fresh and resumed reviews alike. Do not run `aida review record`. If a legacy record attempt was refused, still write this draft. The parent validates and records it under this launch's reviewer grant. Contract: {{\"version\":1,\"pr\":{},\"reviewed_sha\":\"{}\",\"verdict\":\"approved|request-changes|rejected|waiting\",\"summary\":\"complete summary\",\"findings\":[\"complete finding text\"]}}. Include every finding. No additional fields, recorder identity, grant, attestation, receipt, covered specs, commands or ledger paths. Review exactly the named full SHA. Once the draft is written, give a brief final response and exit. Never claim the parent has recorded it.", self.path.display(), self.pr, self.head)
    }

    pub(crate) fn read(&self) -> Result<ReviewerDraft> {
        let file = rv::open_verdict_entry(&self.dir, std::ffi::OsStr::new("draft.json"), 0)
            .context("missing or unreadable reviewer draft")?;
        let meta = file.metadata()?;
        ensure!(
            meta.is_file() && meta.nlink() == 1 && meta.len() <= LIMIT,
            "rejected reviewer draft: expected bounded singly linked regular file"
        );
        ensure!(
            meta.modified()? >= self.since,
            "rejected stale launch draft"
        );
        let mut body = String::new();
        file.take(LIMIT + 1).read_to_string(&mut body)?;
        ensure!(
            body.len() as u64 <= LIMIT,
            "reviewer draft exceeds size limit"
        );
        let draft: ReviewerDraft =
            serde_json::from_str(&body).context("rejected malformed reviewer draft")?;
        ensure!(
            draft.version == 1 && draft.pr == self.pr,
            "reviewer draft version or PR mismatch"
        );
        ensure!(
            full_sha(&draft.reviewed_sha) && draft.reviewed_sha == self.head,
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
        let from = c"draft.json";
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

    /// Rebind a retained handoff's identity (its draft launch id and digest)
    /// after the retained draft's digest was verified against its manifest.
    // trace:BUG-2564 | ai:claude
    fn with_retained(mut self, launch: &str, digest: &str) -> Self {
        self.handoff = Some((launch.to_string(), digest.to_string()));
        self
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

// ── Retained handoff: a validated draft whose recording failed ──────────
//
// After a terminal post-validation failure (record, ledger, verify or
// delivery), the launcher keeps the draft and the IDs of the grant context it
// was validated under, keyed to (PR, head), in its own project root (the
// review worktree is reaped). The next review launch for that PR replays it
// BEFORE it issues any reviewer grant, so a retry never mints a second grant
// for the same draft. Anything unexpected is discarded with a recorded
// reason and the launch runs a fresh review; nothing is ever guessed.
// trace:BUG-2564 | ai:claude

const HANDOFFS: &str = ".aida/review-handoffs";
const MAX_HANDOFF_ATTEMPTS: u32 = 3;

/// The record a retained handoff carries beside its draft: identities only,
/// never a secret.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct HandoffManifest {
    version: u32,
    pr: u64,
    head: String,
    launch: String,
    sha256: String,
    child_grant: String,
    launcher_grant: String,
    session: String,
    since_unix_nanos: u64,
    spec_ids: Vec<String>,
    reason: String,
    attempts: u32,
}

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

fn handoff_base(root: &Path, pr: u64) -> PathBuf {
    root.join(HANDOFFS).join(format!("PR-{pr}"))
}

/// An exclusive per-PR lock: two launchers never replay one handoff.
fn lock_handoffs(base: &Path) -> Result<File> {
    use std::os::fd::AsRawFd;
    std::fs::create_dir_all(base)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(base.join(".lock"))?;
    ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0,
        "cannot lock the retained review handoffs: {}",
        std::io::Error::last_os_error()
    );
    Ok(lock)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// A private file this user owns: regular, singly linked, mode 0600, bounded.
fn read_private(path: &Path) -> Result<Vec<u8>> {
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("missing retained file {}", path.display()))?;
    ensure!(
        meta.is_file() && meta.nlink() == 1 && meta.len() <= LIMIT,
        "retained file {} is not a bounded singly linked regular file",
        path.display()
    );
    ensure!(
        meta.mode() & 0o777 == 0o600 && meta.uid() == unsafe { libc::geteuid() },
        "retained file {} is not mode 0600 owned by this user",
        path.display()
    );
    let mut body = Vec::new();
    File::open(path)?.take(LIMIT + 1).read_to_end(&mut body)?;
    ensure!(
        body.len() as u64 <= LIMIT,
        "retained file exceeds size limit"
    );
    Ok(body)
}

fn stamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%S%.6fZ").to_string()
}

/// Move a retained entry aside with its reason; it is never replayed again.
fn discard(base: &Path, name: &str, reason: &str) {
    let from = base.join(name);
    let to = base.join(format!("discarded-{}-{name}", stamp()));
    if std::fs::rename(&from, &to).is_ok() {
        let _ = write_private(
            &to.join("discard.json"),
            serde_json::json!({"reason": reason, "at": chrono::Utc::now().to_rfc3339()})
                .to_string()
                .as_bytes(),
        );
    } else {
        let _ = std::fs::remove_dir_all(&from);
    }
    eprintln!("  ℹ discarded the retained review handoff {name} for this PR: {reason}");
}

/// Keep a validated draft whose recording failed, for the next launch.
// trace:BUG-2564 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn retain_failed_handoff(
    root: &Path,
    launch: &LaunchDraft,
    draft: &ReviewerDraft,
    reviewer: &VerifiedReviewer,
    spec_ids: &[String],
    since: SystemTime,
    reason: &str,
    attempts: u32,
) -> Result<PathBuf> {
    let (id, digest) = reviewer
        .handoff
        .as_ref()
        .context("only a stamped draft handoff can be retained")?;
    let base = handoff_base(root, launch.pr);
    let _lock = lock_handoffs(&base)?;
    let manifest = HandoffManifest {
        version: 1,
        pr: launch.pr,
        head: launch.head.clone(),
        launch: id.clone(),
        sha256: digest.clone(),
        child_grant: reviewer.grant.id.clone(),
        launcher_grant: reviewer.launcher.clone(),
        session: reviewer.session.clone(),
        since_unix_nanos: since
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_nanos()
            .try_into()?,
        spec_ids: spec_ids.to_vec(),
        reason: reason.to_string(),
        attempts,
    };
    let tmp = base.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(&tmp)?;
    }
    write_private(&tmp.join("draft.json"), &serde_json::to_vec(draft)?)?;
    write_private(
        &tmp.join("handoff.json"),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    let dest = base.join(&launch.head);
    if dest.exists() {
        discard(
            &base,
            &launch.head,
            "superseded by a newer failed handoff for the same head",
        );
    }
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

/// What the next launch found for its PR.
pub(crate) enum RetainedReplay {
    /// Nothing retained (or everything discarded): run a fresh review.
    Fresh,
    /// The retained draft is now recorded: no reviewer is launched.
    Recorded(rv::CollectedVerdict),
}

/// Load one retained entry, enforcing every invariant (C1); `Err` discards.
fn load_retained(dir: &Path, pr: u64, head: &str) -> Result<(HandoffManifest, ReviewerDraft)> {
    use sha2::{Digest, Sha256};
    let manifest: HandoffManifest =
        serde_json::from_slice(&read_private(&dir.join("handoff.json"))?)
            .context("malformed retained handoff manifest")?;
    ensure!(
        manifest.version == 1 && manifest.pr == pr && manifest.head == head && full_sha(head),
        "retained handoff manifest does not name this PR and head"
    );
    let body = read_private(&dir.join("draft.json"))?;
    let draft: ReviewerDraft =
        serde_json::from_slice(&body).context("malformed retained reviewer draft")?;
    let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&draft)?));
    ensure!(
        digest == manifest.sha256,
        "retained draft digest does not match its handoff (tampered or corrupt)"
    );
    ensure!(
        draft.version == 1 && draft.pr == pr && draft.reviewed_sha == head,
        "retained draft does not match its handoff's PR and head"
    );
    ensure!(
        !draft.summary.trim().is_empty()
            && rv::VerdictKind::parse(&draft.verdict) != rv::VerdictKind::Unknown
            && draft.findings.iter().all(|f| !f.trim().is_empty()),
        "retained draft is incomplete"
    );
    Ok((manifest, draft))
}

/// Before any reviewer grant is issued: replay a retained handoff for this
/// PR at its current head, discard stale or unexpected ones with a reason.
/// `Err` means a valid retained draft still cannot be recorded (the ledger
/// is still failing): the launch refuses rather than start a second review.
// trace:BUG-2564 | ai:claude
pub(crate) fn replay_retained_handoff(
    root: &Path,
    pr: u64,
    current_head: Option<&str>,
    verdict_path: &Path,
) -> Result<RetainedReplay> {
    replay_retained_handoff_with(
        root,
        pr,
        current_head,
        verdict_path,
        |spec, draft, reviewer| {
            crate::record_reviewer_draft(root.to_path_buf(), spec, draft, reviewer)
        },
    )
}

/// [`replay_retained_handoff`] with the recorder injected (fixtures pin the
/// head without a forge).
fn replay_retained_handoff_with(
    root: &Path,
    pr: u64,
    current_head: Option<&str>,
    verdict_path: &Path,
    record: impl Fn(&str, &ReviewerDraft, &VerifiedReviewer) -> Result<()>,
) -> Result<RetainedReplay> {
    prune_finished_handoffs(root, pr);
    let base = handoff_base(root, pr);
    if !base.is_dir() {
        return Ok(RetainedReplay::Fresh);
    }
    let _lock = lock_handoffs(&base)?;
    let mut names: Vec<String> = std::fs::read_dir(&base)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n != ".lock" && !n.starts_with("recorded-") && !n.starts_with("discarded-"))
        .collect();
    names.sort();
    let mut replayable = None;
    for name in names {
        if name.starts_with(".tmp-") || !full_sha(&name) || !base.join(&name).is_dir() {
            discard(&base, &name, "unexpected retained handoff entry");
        } else if current_head != Some(name.as_str()) {
            let now = current_head.unwrap_or("an unknown head");
            discard(&base, &name, &format!("head advanced {name} -> {now}"));
        } else {
            replayable = Some(name);
        }
    }
    let Some(head) = replayable else {
        return Ok(RetainedReplay::Fresh);
    };
    let dir = base.join(&head);
    let (manifest, draft) = match load_retained(&dir, pr, &head) {
        Ok(loaded) => loaded,
        Err(e) => {
            discard(&base, &head, &format!("{e:#}"));
            return Ok(RetainedReplay::Fresh);
        }
    };
    let since = SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(manifest.since_unix_nanos);
    let reviewer = match VerifiedReviewer::from_launch(
        root,
        &manifest.child_grant,
        &manifest.launcher_grant,
        since,
        &manifest.session,
    ) {
        Ok(r) => r.with_retained(&manifest.launch, &manifest.sha256),
        Err(e) => {
            discard(
                &base,
                &head,
                &format!("reviewer grant context no longer valid: {e:#}"),
            );
            return Ok(RetainedReplay::Fresh);
        }
    };
    let checks = crate::queue_cmd::StandaloneVerdictChecks {
        project_root: root,
        pr,
        head_sha: Some(head.clone()),
        launcher_grant: Some(manifest.launcher_grant.clone()),
        child_grant: Some(manifest.child_grant.clone()),
    };
    if let Err(e) = rv::check_draft_legacy_conflicts(
        root,
        root,
        verdict_path,
        &manifest.spec_ids,
        since,
        &draft.facts(),
        reviewer.own_handoff().as_ref(),
        &checks,
    ) {
        discard(&base, &head, &format!("{e:#}"));
        return Ok(RetainedReplay::Fresh);
    }
    let labels = if manifest.spec_ids.is_empty() {
        vec![format!("PR-{pr}")]
    } else {
        manifest.spec_ids.clone()
    };
    for spec in &labels {
        record(spec, &draft, &reviewer)
            .context("retained review handoff is still not recordable; retry the launch later")?;
    }
    let collected =
        rv::verify_recorded_draft(root, verdict_path, &manifest.spec_ids, since, &checks)
            .map_err(anyhow::Error::msg)
            .context("retained review handoff is still not verifiable; retry the launch later")?;
    let _ = std::fs::rename(&dir, base.join(format!("recorded-{}-{head}", stamp())));
    Ok(RetainedReplay::Recorded(collected))
}

/// C4: retained handoffs end with their change. Remove every PR's handoff
/// directory (other than `keep`) whose change the forge reports merged or
/// closed. Best-effort; a forge miss keeps the directory.
// trace:BUG-2564 | ai:claude
fn prune_finished_handoffs(root: &Path, keep: u64) {
    let Ok(entries) = std::fs::read_dir(root.join(HANDOFFS)) else {
        return;
    };
    let kind = crate::forge::resolve_open_change_forge_kind(root);
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(n) = name.strip_prefix("PR-").and_then(|n| n.parse::<u64>().ok()) else {
            continue;
        };
        if n == keep {
            continue;
        }
        let finished = crate::merge_hold::fetch_pinned_change(root, kind, n)
            .ok()
            .flatten()
            .is_some_and(|c| {
                matches!(
                    c.state,
                    crate::merge_hold::ChangeState::Merged
                        | crate::merge_hold::ChangeState::ClosedUnmerged
                )
            });
        if finished {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
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
        let launch = LaunchDraft::prepare(tmp.path(), 59, HEAD.into()).unwrap();
        (tmp, launch)
    }
    fn write(launch: &LaunchDraft, value: &serde_json::Value) {
        std::fs::write(&launch.path, serde_json::to_vec(value).unwrap()).unwrap();
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
            assert!(rendered.contains(launch.path.to_str().unwrap()));
            assert!(rendered.contains("Do not run `aida review record`"));
            assert!(rendered.contains("still write this draft"));
        }
    }
    #[test]
    fn standalone_review_verdict_draft_rejects_wrong_missing_and_authority_fields() {
        let (_tmp, launch) = fixture();
        assert!(LaunchDraft::prepare(_tmp.path(), 59, String::new()).is_err());
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
        std::fs::write(&launch.path, b"{\"version\":1,\"version\":1}").unwrap();
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
        std::fs::write(&launch.path, b"{broken").unwrap();
        assert!(launch.read().is_err());
        drop(done);
        watchdog.join().unwrap();
    }
    #[test]
    fn standalone_review_verdict_draft_pins_directory_and_excludes_old_launches() {
        let (tmp, first) = fixture();
        write(&first, &payload());
        let second = LaunchDraft::prepare(tmp.path(), 59, HEAD.into()).unwrap();
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
        let launch = LaunchDraft::prepare(&f.root, 59, f.head.clone()).unwrap();
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
        std::fs::write(&launch.path, body.to_string()).unwrap();
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
        let launch = LaunchDraft::prepare(&f.root, 59, f.head.clone()).unwrap();
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
    /// C2: a record from another launch conflicts even when it was made by
    /// the same reviewer seat.
    // trace:BUG-2564 | ai:claude
    #[test]
    fn standalone_review_verdict_parent_other_launch_same_seat_still_conflicts() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let first = LaunchDraft::prepare(&f.root, 59, f.head.clone()).unwrap();
        let draft = approved_with_deferred(&f, &first);
        let own = f.reviewer().unwrap().with_draft(&first, &draft).unwrap();
        let other = LaunchDraft::prepare(&f.root, 59, f.head.clone()).unwrap();
        std::fs::write(
            &other.path,
            serde_json::json!({"version":1,"pr":59,"reviewed_sha":f.head,"verdict":"request-changes",
                "summary":"another launch","findings":["blocking defect"]})
            .to_string(),
        )
        .unwrap();
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
    fn grant_count() -> usize {
        std::fs::read_dir(sa::grant_dir().unwrap())
            .map(|d| d.count())
            .unwrap_or(0)
    }
    /// A launch whose delivery failed retains its validated draft; returns the
    /// retained directory.
    fn retained(f: &Fixture) -> (PathBuf, ReviewerDraft) {
        let launch = LaunchDraft::prepare(&f.root, 59, f.head.clone()).unwrap();
        let draft = approved_with_deferred(f, &launch);
        let reviewer = f.reviewer().unwrap().with_draft(&launch, &draft).unwrap();
        crate::gate::ledger::FAIL_NEXT_FINDING_DELIVERY.with(|c| c.set(true));
        let err = f
            .record(&draft.verdict, &draft.findings, Some(&reviewer))
            .unwrap_err();
        let dir = retain_failed_handoff(
            &f.root,
            &launch,
            &draft,
            &reviewer,
            &["STORY-77".into()],
            f.since,
            &format!("{err:#}"),
            3,
        )
        .unwrap();
        (dir, draft)
    }
    fn replay(f: &Fixture, head: &str) -> RetainedReplay {
        replay_retained_handoff_with(
            &f.root,
            59,
            Some(head),
            &rv::verdict_path(&f.root, "PR-59"),
            |_, draft, reviewer| f.record(&draft.verdict, &draft.findings, Some(reviewer)),
        )
        .unwrap()
    }
    fn discard_reasons(f: &Fixture) -> Vec<String> {
        let base = f.root.join(".aida/review-handoffs/PR-59");
        std::fs::read_dir(&base)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("discarded-"))
            .map(|e| std::fs::read_to_string(e.path().join("discard.json")).unwrap())
            .collect()
    }
    /// P1-1: the next launch records a retained draft before issuing any
    /// grant, and completes the deferred approval.
    // trace:BUG-2564 | ai:claude
    #[test]
    fn retained_handoff_replays_on_next_launch_without_a_new_grant() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let (dir, _) = retained(&f);
        let meta = std::fs::metadata(dir.join("draft.json")).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o600);
        let grants = grant_count();
        let RetainedReplay::Recorded(collected) = replay(&f, &f.head) else {
            panic!("the retained draft was not replayed");
        };
        assert_eq!(collected.verdict.kind, rv::VerdictKind::Approved);
        assert_eq!(grant_count(), grants, "replay minted no grant");
        let recorded = rv::read_recorded_verdict(&f.root, "PR-59").unwrap();
        assert_eq!(recorded.kind, rv::VerdictKind::Approved);
        assert_eq!(recorded.reviewed_sha.as_deref(), Some(f.head.as_str()));
        assert!(!dir.exists(), "a recorded handoff is moved aside");
        assert!(
            matches!(replay(&f, &f.head), RetainedReplay::Fresh),
            "replayed once"
        );
    }
    #[test]
    fn retained_handoff_for_an_older_head_is_discarded_with_its_reason() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        retained(&f);
        let newer = "f".repeat(40);
        assert!(matches!(replay(&f, &newer), RetainedReplay::Fresh));
        let reasons = discard_reasons(&f);
        assert!(
            reasons.iter().any(|r| r.contains("head advanced")),
            "{reasons:?}"
        );
    }
    /// C1: a tampered retained draft is never recorded.
    // trace:BUG-2564 | ai:claude
    #[test]
    fn retained_handoff_with_a_tampered_draft_is_refused() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let (dir, draft) = retained(&f);
        let mut changed = serde_json::to_value(&draft).unwrap();
        changed["summary"] = "a summary the reviewer never wrote".into();
        let path = dir.join("draft.json");
        std::fs::remove_file(&path).unwrap();
        write_private(&path, &serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(matches!(replay(&f, &f.head), RetainedReplay::Fresh));
        let reasons = discard_reasons(&f);
        assert!(reasons.iter().any(|r| r.contains("digest")), "{reasons:?}");
        let current = rv::read_recorded_verdict(&f.root, "PR-59").unwrap();
        assert_ne!(
            current.summary.as_deref(),
            Some("a summary the reviewer never wrote")
        );
    }
    #[test]
    fn retained_handoff_with_loose_permissions_or_stray_entries_is_discarded() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        let (dir, _) = retained(&f);
        std::fs::set_permissions(
            dir.join("draft.json"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        std::fs::create_dir(f.root.join(".aida/review-handoffs/PR-59/not-a-head")).unwrap();
        assert!(matches!(replay(&f, &f.head), RetainedReplay::Fresh));
        let reasons = discard_reasons(&f);
        assert!(reasons.iter().any(|r| r.contains("0600")), "{reasons:?}");
        assert!(
            reasons.iter().any(|r| r.contains("unexpected")),
            "{reasons:?}"
        );
    }
    #[test]
    fn retained_handoff_with_a_revoked_grant_is_discarded() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        retained(&f);
        assert!(sa::revoke_by_id(&f.child).unwrap());
        assert!(matches!(replay(&f, &f.head), RetainedReplay::Fresh));
        let reasons = discard_reasons(&f);
        assert!(reasons.iter().any(|r| r.contains("grant")), "{reasons:?}");
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
        let launch = LaunchDraft::prepare(&f.root, 59, f.head.clone()).unwrap();
        let data = serde_json::json!({"version":1,"pr":59,"reviewed_sha":f.head,"verdict":"approved","summary":"complete child rationale","findings":[]});
        std::fs::write(&launch.path, data.to_string()).unwrap();
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
