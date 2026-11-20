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
        let draft = entry.path().join("draft.json");
        match std::fs::symlink_metadata(&draft) {
            Ok(_) => drafts.push(draft),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("cannot inspect {}", draft.display())),
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
            self.record_as(
                "STORY-77",
                verdict,
                "complete child rationale",
                findings,
                reviewer,
            )
        }
        /// [`Self::record`] for any spec PR-59 covers, with its own summary.
        // trace:BUG-2662 | ai:claude
        fn record_as(
            &self,
            spec: &str,
            verdict: &str,
            summary: &str,
            findings: &[String],
            reviewer: Option<&VerifiedReviewer>,
        ) -> Result<()> {
            crate::with_review_pr_subject(
                crate::ReviewPrSubject {
                    head_ref: "feature".into(),
                    commit_subjects: vec!["change (STORY-77)".into(), "more (STORY-78)".into()],
                },
                || {
                    crate::handle_review_record_context_at(
                        self.root.clone(),
                        spec,
                        verdict,
                        Some(&self.head),
                        Some("feature"),
                        Some(summary),
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
        fn backend(&self) -> aida_core::CachedGitBackend {
            let store = crate::detect_distributed_store_from(&self.root).unwrap();
            crate::test_env::home_fixture_cached_backend(
                &store,
                &aida_core::CachedGitBackend::default_cache_path(&store),
            )
            .unwrap()
        }
        /// Every requirement in the store as `(id, description)`, sorted.
        fn store_rows(&self) -> Vec<(String, String)> {
            let mut rows: Vec<_> = self
                .backend()
                .list_requirements(true)
                .unwrap()
                .into_iter()
                .map(|r| (r.id.to_string(), r.description))
                .collect();
            rows.sort();
            rows
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
    /// whose delivery failed. The failed attempt records nothing (BUG-2662),
    /// and retrying the same handoff records the approval at the head.
    // trace:BUG-2564 trace:BUG-2662 | ai:claude
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
        let error = f
            .record(&draft.verdict, &draft.findings, Some(&reviewer))
            .unwrap_err();
        // The top-level message names the cause, not only the failed step.
        let error = error.to_string();
        assert!(
            error.contains("deferred findings were not filed"),
            "{error}"
        );
        assert!(
            crate::gate::ledger::project::verdict_records(&f.root, "PR-59").is_empty(),
            "a failed approval records no verdict"
        );
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
    /// Every verdict event on PR-59's ledger, oldest first.
    // trace:BUG-2662 | ai:claude
    fn verdict_events(f: &Fixture) -> Vec<rv::JsonObj> {
        let store = crate::gate::ledger::store_root(&f.root).unwrap();
        crate::gate::ledger::store::read_file(&store, crate::gate::ledger::ChangeKey::pr(59))
            .iter()
            .filter_map(|e| e.verdict_record().cloned())
            .collect()
    }
    /// Append `record` to PR-59's ledger for STORY-77, as another seat's
    /// earlier write.
    fn seed_verdict(f: &Fixture, record: serde_json::Value) {
        let store = crate::gate::ledger::store_root(&f.root).unwrap();
        let change = crate::gate::ledger::ChangeKey::pr(59);
        let spec = crate::gate::ledger::GateSpec::resolve(&f.root, "STORY-77");
        let event = crate::gate::ledger::event::verdict_event(
            &store,
            change,
            record.as_object().unwrap(),
            Some(&spec),
        );
        crate::gate::ledger::store::append(&store, change, vec![event]).unwrap();
    }
    fn word(record: &rv::JsonObj) -> &str {
        record["verdict"].as_str().unwrap()
    }
    /// One `review record --verdict approved` is exactly one verdict event,
    /// and an identical repeat adds none.
    // trace:BUG-2662 | ai:claude
    #[test]
    fn bug_2662_one_approval_is_one_verdict_event_and_a_repeat_adds_none() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let reviewer = f.reviewer().unwrap();
        f.record("approved", &[], Some(&reviewer)).unwrap();
        let events = verdict_events(&f);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(word(&events[0]), "approved");
        // The one record is the handshake the orchestrator reads.
        assert_eq!(events[0]["mode"], crate::gate::ledger::project::PHASE3_MODE);
        assert!(events[0]["release_grant"].is_object());
        f.record("approved", &[], Some(&reviewer)).unwrap();
        assert_eq!(
            verdict_events(&f),
            events,
            "an unchanged record writes nothing"
        );
    }
    /// The PR #4218 shape: the standing record is another reviewer's
    /// delivery-pending barrier (shown as CHANGES REQUESTED) carrying a
    /// deferred finding. One approval that inherits the finding appends one
    /// approved event and no request-changes event; a repeat adds none.
    // trace:BUG-2662 | ai:claude
    #[test]
    fn bug_2662_approval_over_pending_barrier_writes_one_approved_event() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let finding: serde_json::Value = serde_json::from_str(&deferred(1)).unwrap();
        seed_verdict(
            &f,
            serde_json::json!({"verdict":"request-changes","sweep_pending":true,
                "reviewed_sha":f.head,"recorded_by":"codex reviewer seat",
                "recorded_at":"2026-11-17T17:40:00+00:00","summary":"barrier",
                "findings":[finding["text"]],
                "finding_dispositions":[{"severity":finding["severity"],
                    "disposition":finding["disposition"],"rationale":finding["rationale"]}]}),
        );
        assert_eq!(
            rv::read_recorded_verdict(&f.root, "PR-59").unwrap().kind,
            rv::VerdictKind::RequestChanges
        );
        let reviewer = f.reviewer().unwrap();
        f.record("approved", &[], Some(&reviewer)).unwrap();
        let events = verdict_events(&f);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(word(&events[1]), "approved");
        assert_ne!(events[1].get("sweep_pending"), Some(&true.into()));
        assert_eq!(events[1]["findings"], serde_json::json!(["P3 follow-up 1"]));
        let recorded = rv::read_recorded_verdict(&f.root, "PR-59").unwrap();
        assert_eq!(recorded.kind, rv::VerdictKind::Approved);
        f.record("approved", &[], Some(&reviewer)).unwrap();
        assert_eq!(
            verdict_events(&f),
            events,
            "an unchanged record writes nothing"
        );
    }
    /// The PR #4290 shape: a reviewer's request-changes stands at the head and
    /// an approval carrying a deferred finding cannot file it. The refused
    /// approval writes no event (the refusal on record is still the other
    /// reviewer's), the error names the cause, and a retry records once.
    // trace:BUG-2662 | ai:claude
    #[test]
    fn bug_2662_refused_approval_over_standing_refusal_writes_nothing() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        seed_verdict(
            &f,
            serde_json::json!({"verdict":"request-changes","reviewed_sha":f.head,
                "recorded_by":"codex reviewer seat",
                "recorded_at":"2026-11-19T13:00:00+00:00","summary":"one blocker",
                "findings":["blocker"]}),
        );
        let seeded = verdict_events(&f);
        let reviewer = f.reviewer().unwrap();
        crate::gate::ledger::FAIL_NEXT_FINDING_DELIVERY.with(|c| c.set(true));
        let error = f
            .record("approved", &[deferred(1)], Some(&reviewer))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("could not write the review verdict"),
            "{error}"
        );
        assert!(
            error.contains("deferred findings were not filed"),
            "{error}"
        );
        assert_eq!(
            verdict_events(&f),
            seeded,
            "a refused approval writes nothing"
        );
        let standing = rv::read_recorded_verdict(&f.root, "PR-59").unwrap();
        assert_eq!(standing.kind, rv::VerdictKind::RequestChanges);
        assert_eq!(standing.recorded_by.as_deref(), Some("codex reviewer seat"));
        f.record("approved", &[deferred(1)], Some(&reviewer))
            .unwrap();
        let events = verdict_events(&f);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(word(&events[1]), "approved");
    }
    /// Every event on PR-59's ledger, of any kind, oldest first.
    // trace:BUG-2662 | ai:claude
    fn ledger_events(f: &Fixture) -> Vec<crate::gate::ledger::GateEvent> {
        let store = crate::gate::ledger::store_root(&f.root).unwrap();
        crate::gate::ledger::store::read_file(&store, crate::gate::ledger::ChangeKey::pr(59))
    }
    /// PR-59's ledger file, byte for byte.
    fn ledger_bytes(f: &Fixture) -> Vec<u8> {
        let store = crate::gate::ledger::store_root(&f.root).unwrap();
        std::fs::read(crate::gate::ledger::store::ledger_file(
            &store,
            crate::gate::ledger::ChangeKey::pr(59),
        ))
        .unwrap()
    }
    /// The spec labels of PR-59's verdict events, oldest first.
    fn verdict_labels(f: &Fixture) -> Vec<Option<String>> {
        ledger_events(f)
            .into_iter()
            .filter(|e| e.verdict_record().is_some())
            .map(|e| e.spec)
            .collect()
    }
    /// How many review-verdict coordination events the fixture has emitted.
    fn announced(f: &Fixture) -> usize {
        crate::events::read_all(&f.root)
            .iter()
            .filter(|e| {
                matches!(
                    e.kind,
                    crate::events::EventKind::ReviewVerdictRecorded { .. }
                )
            })
            .count()
    }
    /// Run `hook` once, as a writer racing the next review append: after the
    /// command made its reads and before the append takes its locks.
    fn race_next_append(hook: impl Fn() + 'static) -> impl Drop {
        struct Unhook;
        impl Drop for Unhook {
            fn drop(&mut self) {
                crate::gate::ledger::store::REVIEW_APPEND_HOOK.with(|h| h.borrow_mut().take());
            }
        }
        let fired = std::cell::Cell::new(false);
        crate::gate::ledger::store::REVIEW_APPEND_HOOK.with(|h| {
            *h.borrow_mut() = Some(Box::new(move |_, locked| {
                if !locked && !fired.replace(true) {
                    hook();
                }
            }))
        });
        Unhook
    }
    /// A PR covering two specs: after STORY-77 is approved, the same
    /// reviewer's same summary, head and findings for STORY-78 is that spec's
    /// first review, not a repeat. It gets its own event, binding and
    /// follow-up delivery.
    // trace:BUG-2662 | ai:claude
    #[test]
    fn bug_2662_same_review_for_a_sibling_spec_is_not_a_repeat() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let mut sibling = aida_core::Requirement::new("sibling change".into(), "fixture".into());
        sibling.spec_id = Some("STORY-78".into());
        f.backend().add_requirement(sibling).unwrap();
        let reviewer = f.reviewer().unwrap();
        let approve = |spec: &str| {
            f.record_as(
                spec,
                "approved",
                "complete child rationale",
                &[deferred(1)],
                Some(&reviewer),
            )
        };
        approve("STORY-77").unwrap();
        assert_eq!(verdict_labels(&f), [Some("STORY-77".to_string())]);
        let filed_for_first = f.store_rows();
        approve("STORY-78").unwrap();
        assert_eq!(
            verdict_labels(&f),
            [Some("STORY-77".to_string()), Some("STORY-78".to_string())],
            "the sibling spec's first review is recorded"
        );
        let store = crate::gate::ledger::store_root(&f.root).unwrap();
        assert!(
            crate::gate::ledger::project::changes_for_spec(&store, "STORY-78")
                .contains(&crate::gate::ledger::ChangeKey::pr(59)),
            "the sibling spec is bound to the change"
        );
        assert!(
            rv::read_recorded_verdict(&f.root, "STORY-78")
                .unwrap()
                .kind
                .approves(),
            "the sibling spec answers for itself"
        );
        let filed_for_both = f.store_rows();
        assert_ne!(
            filed_for_both, filed_for_first,
            "the sibling spec's follow-up is filed under its own source"
        );
        // STORY-78's approval now stands for the change: its repeat writes
        // nothing, and STORY-77's is a new round rather than "unchanged".
        let before = ledger_bytes(&f);
        approve("STORY-78").unwrap();
        assert_eq!(ledger_bytes(&f), before);
        assert_eq!(f.store_rows(), filed_for_both);
        approve("STORY-77").unwrap();
        assert_eq!(verdict_labels(&f).len(), 3);
    }
    /// A refusal that carries a deferred follow-up beside a blocking finding
    /// is one verdict event however its filing goes: a retry after a failed
    /// filing, and a repeat after a successful one, file against the refusal
    /// that stands and write no second event.
    // trace:BUG-2662 | ai:claude
    #[test]
    fn bug_2662_mixed_refusal_retry_keeps_one_verdict_event() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let config = f.root.join(".aida/config.toml");
        let original = std::fs::read_to_string(&config).unwrap_or_default();
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            format!("{original}\n[review]\nblocking_tiers = [\"p1\"]\n"),
        )
        .unwrap();
        crate::review_blocking::commit_config_for_test(&f.root);
        let findings = [
            serde_json::json!({"text":"blocking defect","severity":"major","blocking_tier":"p1"})
                .to_string(),
            crate::review_blocking::tests::p2_finding(),
        ];
        let reviewer = f.reviewer().unwrap();
        let refuse = || f.record("request-changes", &findings, Some(&reviewer));
        let rows_before = f.store_rows();
        crate::gate::ledger::FAIL_NEXT_FINDING_DELIVERY.with(|c| c.set(true));
        let error = refuse().unwrap_err().to_string();
        assert!(error.contains("deferred findings pending"), "{error}");
        let events = verdict_events(&f);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(word(&events[0]), "request-changes");
        assert_eq!(f.store_rows(), rows_before, "the filing failed");
        assert_eq!(announced(&f), 0, "the failed recording announced nothing");
        // The retry files against the refusal on record and protects the PR.
        refuse().unwrap();
        assert_eq!(announced(&f), 1, "the completed recording is announced");
        assert_eq!(
            verdict_events(&f),
            events,
            "a retried filing writes no second verdict event"
        );
        let rows_filed = f.store_rows();
        assert!(
            rows_filed.len() > rows_before.len(),
            "the follow-up is filed"
        );
        assert!(crate::gate::merge_hold_core::read_hold(&f.root, 59).is_some());
        // A repeat after a successful filing changes nothing at all.
        let ledger = ledger_bytes(&f);
        refuse().unwrap();
        assert_eq!(ledger_bytes(&f), ledger);
        assert_eq!(f.store_rows().len(), rows_filed.len());
        assert_eq!(announced(&f), 1, "a repeat announces nothing");
    }
    /// A repeated refusal leaves the whole change ledger as it was: no second
    /// verdict event and, while its hold stands, no second hold event. When
    /// the hold is gone, the repeat restores it.
    // trace:BUG-2662 | ai:claude
    #[test]
    fn bug_2662_repeated_refusal_leaves_the_change_ledger_unchanged() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let reviewer = f.reviewer().unwrap();
        let refuse = || f.record("request-changes", &["blocker".into()], Some(&reviewer));
        refuse().unwrap();
        let holds = |f: &Fixture| {
            ledger_events(f)
                .iter()
                .filter(|e| matches!(e.body, crate::gate::ledger::EventBody::Hold { .. }))
                .count()
        };
        assert_eq!(verdict_events(&f).len(), 1);
        assert_eq!(holds(&f), 1);
        assert_eq!(announced(&f), 1);
        let ledger = ledger_bytes(&f);
        for _ in 0..2 {
            refuse().unwrap();
            assert_eq!(announced(&f), 1, "a repeat announces nothing");
            assert_eq!(
                ledger_bytes(&f),
                ledger,
                "a repeated refusal writes no verdict, hold or label event"
            );
        }
        // Protection lost since the refusal is reconciled by its repeat.
        let clearance = serde_json::json!({"cleared_by":"test","reason":"simulated loss"});
        crate::gate::ledger::hold::release(&f.root, 59, clearance.as_object().unwrap().clone())
            .unwrap();
        assert!(crate::gate::merge_hold_core::read_hold(&f.root, 59).is_none());
        refuse().unwrap();
        assert!(crate::gate::merge_hold_core::read_hold(&f.root, 59).is_some());
        assert_eq!(verdict_events(&f).len(), 1, "still one verdict event");
        assert_eq!(holds(&f), 2);
    }
    /// A refusal recorded after a repeat made its reads and before its append
    /// took the ledger locks: the repeat is decided under those locks against
    /// the act standing then, so it is never reported as unchanged. A plain
    /// approval is a new round; one with follow-ups already filed is refused.
    // trace:BUG-2662 | ai:claude
    #[test]
    fn bug_2662_repeat_is_revalidated_under_the_ledger_lock() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        for findings in [vec![], vec![deferred(1)]] {
            let f = Fixture::new();
            let reviewer = f.reviewer().unwrap();
            f.record("approved", &findings, Some(&reviewer)).unwrap();
            assert_eq!(verdict_events(&f).len(), 1);
            let root = f.root.clone();
            let head = f.head.clone();
            let _race = race_next_append(move || {
                let store = crate::gate::ledger::store_root(&root).unwrap();
                let change = crate::gate::ledger::ChangeKey::pr(59);
                let refusal = serde_json::json!({"verdict":"request-changes",
                    "reviewed_sha":head,"recorded_by":"codex reviewer seat",
                    "recorded_at":"2026-11-20T19:00:00+00:00","findings":["blocker"]});
                let event = crate::gate::ledger::event::verdict_event(
                    &store,
                    change,
                    refusal.as_object().unwrap(),
                    None,
                );
                crate::gate::ledger::store::append(&store, change, vec![event]).unwrap();
            });
            let repeated = f.record("approved", &findings, Some(&reviewer));
            let words: Vec<String> = verdict_events(&f)
                .iter()
                .map(|r| word(r).to_string())
                .collect();
            if findings.is_empty() {
                repeated.unwrap();
                assert_eq!(words, ["approved", "request-changes", "approved"]);
            } else {
                let error = repeated.unwrap_err().to_string();
                assert!(error.contains("another review was recorded"), "{error}");
                assert_eq!(words, ["approved", "request-changes"]);
                assert_eq!(
                    rv::read_recorded_verdict(&f.root, "PR-59").unwrap().kind,
                    rv::VerdictKind::RequestChanges
                );
            }
        }
    }
    /// The human-review receipts in the fixture's AIDA home, sorted by id.
    // trace:BUG-2662 | ai:claude
    fn receipts(f: &Fixture) -> Vec<String> {
        let dir = f.tmp.path().join("home/.aida/review-receipts");
        let mut ids: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|e| {
                        e.path()
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                    })
                    .collect()
            })
            .unwrap_or_default();
        ids.sort();
        ids
    }
    /// The receipt each of PR-59's verdict events names, oldest first.
    fn event_receipts(f: &Fixture) -> Vec<String> {
        verdict_events(f)
            .iter()
            .map(|r| {
                r[rv::ATTESTATION_KEY]["receipt_id"]
                    .as_str()
                    .expect("an operator approval names its receipt")
                    .to_string()
            })
            .collect()
    }
    /// The operator at a terminal (the PR #4218 recorder). Two approvals that
    /// differ are two events with their own receipts. A repeat answers with
    /// the standing receipt: no event and no second receipt. When that
    /// receipt is missing or no longer vouches for the commit, the repeat is
    /// a new round under a new receipt, which is the one the record names.
    // trace:BUG-2662 | ai:claude
    #[test]
    fn bug_2662_operator_repeat_reuses_a_valid_standing_receipt() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let mut f = Fixture::new();
        f._env.unset_key("AIDA_AGENT_TYPE");
        f._env.unset_key("AIDA_AGENT_NAME");
        let approve = |summary: &str| {
            let source = ra::IdentitySource {
                session_ids: vec!["operator-terminal-session".into()],
                ..Default::default()
            };
            ra::with_identity_source(source, || {
                crate::gate::solo_review::with_test_human_terminal("", || {
                    f.record_as("STORY-77", "approved", summary, &[], None)
                })
            })
        };
        let valid = |id: &str| ra::human_receipt_for(id, &f.head).is_some();
        approve("first pass").unwrap();
        let first = event_receipts(&f);
        assert_eq!(first.len(), 1);
        assert_eq!(receipts(&f), first, "one approval, one receipt");
        let recorded = rv::read_recorded_verdict(&f.root, "PR-59").unwrap();
        assert!(recorded.attestation.unwrap().human_at_tty);

        // A repeat: no event, no receipt left behind.
        approve("first pass").unwrap();
        assert_eq!(event_receipts(&f), first);
        assert_eq!(receipts(&f), first, "a repeat allocates no receipt");

        // A second, different approval: its own event and receipt.
        approve("second pass").unwrap();
        let two = event_receipts(&f);
        assert_eq!(two.len(), 2);
        assert_ne!(two[0], two[1], "distinct receipts");
        assert_eq!(receipts(&f).len(), 2);
        assert!(valid(&two[0]) && valid(&two[1]));
        approve("second pass").unwrap();
        assert_eq!(event_receipts(&f), two);
        assert_eq!(receipts(&f).len(), 2);

        // The standing receipt is missing: the repeat is a new round under a
        // new receipt, the only one that vouches for the standing record.
        let dir = f.tmp.path().join("home/.aida/review-receipts");
        std::fs::remove_file(dir.join(format!("{}.json", two[1]))).unwrap();
        approve("second pass").unwrap();
        let three = event_receipts(&f);
        assert_eq!(three.len(), 3);
        assert_eq!(three[..2], two[..]);
        assert!(!two.contains(&three[2]));
        assert!(valid(&three[2]) && !valid(&two[1]));
        assert_eq!(receipts(&f).len(), 2, "the lost receipt is replaced once");

        // The standing receipt no longer vouches for this commit.
        let path = dir.join(format!("{}.json", three[2]));
        let mut receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        receipt["sha"] = "0".repeat(40).into();
        std::fs::write(&path, receipt.to_string()).unwrap();
        assert!(!valid(&three[2]));
        approve("second pass").unwrap();
        let four = event_receipts(&f);
        assert_eq!(four.len(), 4);
        assert_eq!(four[..3], three[..]);
        assert!(valid(&four[3]), "the newest record names a valid receipt");
        let standing = rv::read_recorded_verdict(&f.root, "PR-59").unwrap();
        assert_eq!(
            standing.attestation.unwrap().receipt_id.as_deref(),
            Some(four[3].as_str())
        );
        // And that record repeats without another receipt.
        let count = receipts(&f).len();
        approve("second pass").unwrap();
        assert_eq!(event_receipts(&f), four);
        assert_eq!(receipts(&f).len(), count);
    }
    /// Run `f` as the operator at a terminal (the PR #4218 recorder).
    // trace:BUG-2662 | ai:claude
    fn as_operator<R>(f: impl FnOnce() -> R) -> R {
        let source = ra::IdentitySource {
            session_ids: vec!["operator-terminal-session".into()],
            ..Default::default()
        };
        ra::with_identity_source(source, || {
            crate::gate::solo_review::with_test_human_terminal("", f)
        })
    }
    /// A human-at-TTY approval that is not recorded leaves no receipt: not
    /// when its follow-up findings cannot be filed (the receipt is not yet
    /// issued), and not when the ledger write fails after the receipt was
    /// issued under the lock (the record is proved absent, so it is removed
    /// again). The retry that lands
    /// names the one receipt that exists.
    // trace:BUG-2662 | ai:claude
    #[test]
    fn bug_2662_unrecorded_operator_approval_leaves_no_receipt() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let mut f = Fixture::new();
        f._env.unset_key("AIDA_AGENT_TYPE");
        f._env.unset_key("AIDA_AGENT_NAME");
        let approve = |findings: &[String]| {
            as_operator(|| f.record_as("STORY-77", "approved", "first pass", findings, None))
        };
        for findings in [vec![deferred(1)], vec![]] {
            if !findings.is_empty() {
                crate::gate::ledger::FAIL_NEXT_FINDING_DELIVERY.with(|c| c.set(true));
                let error = approve(&findings).unwrap_err().to_string();
                assert!(
                    error.contains("deferred findings were not filed"),
                    "{error}"
                );
                assert!(receipts(&f).is_empty(), "a failed filing issues no receipt");
            }
            let before = verdict_events(&f);
            let kept = receipts(&f);
            crate::gate::ledger::store::FAIL_NEXT_REVIEW_WRITE.with(|c| c.set(true));
            let error = approve(&findings).unwrap_err().to_string();
            assert!(error.contains("injected review write failure"), "{error}");
            assert_eq!(
                verdict_events(&f),
                before,
                "the failed write appended nothing"
            );
            assert_eq!(receipts(&f), kept, "its receipt was removed again");
        }
        assert!(verdict_events(&f).is_empty());
        approve(&[deferred(1)]).unwrap();
        let named = event_receipts(&f);
        assert_eq!(named.len(), 1);
        assert_eq!(receipts(&f), named, "the one receipt is the one on record");
        assert!(ra::human_receipt_for(&named[0], &f.head).is_some());
        let recorded = rv::read_recorded_verdict(&f.root, "PR-59").unwrap();
        assert_eq!(
            recorded.attestation.unwrap().receipt_id.as_deref(),
            Some(named[0].as_str())
        );
    }
    /// How many receipt reservations the fixture's AIDA home holds.
    // trace:BUG-2662 | ai:claude
    fn reservations(f: &Fixture) -> usize {
        std::fs::read_dir(f.tmp.path().join("home/.aida/review-receipt-reservations"))
            .map(|entries| entries.count())
            .unwrap_or(0)
    }
    /// A first PR approval for a spec that already has an unbound verdict
    /// binds that verdict to the change in the same append as the approval:
    /// when the receipt cannot be issued, or the write fails after it was,
    /// the change's ledger is left as it was and no receipt remains; when it
    /// lands, the change holds the bound copy and the one approval.
    // trace:BUG-2662 | ai:claude
    #[test]
    fn bug_2662_binding_and_first_approval_land_together_or_not_at_all() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let mut f = Fixture::new();
        f._env.unset_key("AIDA_AGENT_TYPE");
        f._env.unset_key("AIDA_AGENT_NAME");
        let store = crate::gate::ledger::store_root(&f.root).unwrap();
        let spec = crate::gate::ledger::GateSpec::resolve(&f.root, "STORY-77");
        let earlier = serde_json::json!({"verdict":"request-changes","reviewed_sha":f.head,
            "recorded_by":"reviewer-a","recorded_at":"2026-11-08T00:00:00Z"});
        let earlier = earlier.as_object().unwrap();
        crate::gate::ledger::store::append_unbound(&store, &spec, earlier).unwrap();
        let approve =
            || as_operator(|| f.record_as("STORY-77", "approved", "first pass", &[], None));
        assert!(ledger_events(&f).is_empty());

        // The receipt cannot be issued.
        let blocked = f.tmp.path().join("home/.aida/review-receipt-reservations");
        std::fs::create_dir_all(blocked.parent().unwrap()).unwrap();
        std::fs::write(&blocked, "").unwrap();
        let error = format!("{:#}", approve().unwrap_err());
        assert!(error.contains("human-review receipt"), "{error}");
        assert!(ledger_events(&f).is_empty(), "nothing was bound");
        assert!(receipts(&f).is_empty());
        std::fs::remove_file(&blocked).unwrap();

        // The write fails after the receipt was issued.
        crate::gate::ledger::store::FAIL_NEXT_REVIEW_WRITE.with(|c| c.set(true));
        let error = format!("{:#}", approve().unwrap_err());
        assert!(error.contains("injected review write failure"), "{error}");
        assert!(ledger_events(&f).is_empty(), "nothing was bound");
        assert!(receipts(&f).is_empty());
        assert_eq!(reservations(&f), 0);

        // It lands: the bound copy, then the one approval.
        approve().unwrap();
        let events = verdict_events(&f);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(&events[0], earlier, "the unbound verdict, as it was");
        assert_eq!(word(&events[1]), "approved");
        let named = events[1][rv::ATTESTATION_KEY]["receipt_id"]
            .as_str()
            .unwrap();
        assert_eq!(receipts(&f), [named.to_string()]);
        let standing = rv::read_recorded_verdict(&f.root, "PR-59").unwrap();
        assert_eq!(standing.kind, rv::VerdictKind::Approved);
        let ledger = ledger_bytes(&f);
        approve().unwrap();
        assert_eq!(
            ledger_bytes(&f),
            ledger,
            "a repeat binds and records nothing"
        );
        assert_eq!(receipts(&f).len(), 1);
    }
    /// The ledger write completes and its sync reports an I/O error: the
    /// event may be on the ledger, but it is not confirmed durable, so the
    /// recording is refused, nothing is announced and its receipt is kept.
    /// The retry is a repeat of that act: it syncs the ledger before it
    /// answers (and is refused again while the sync fails), appends no second
    /// event, issues no second receipt and announces the act once.
    // trace:BUG-2662 | ai:claude
    #[test]
    fn bug_2662_approval_whose_sync_failed_is_unconfirmed_until_a_retry_syncs_it() {
        use crate::gate::ledger::store::{ReviewFault, NEXT_REVIEW_FAULT};
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let mut f = Fixture::new();
        f._env.unset_key("AIDA_AGENT_TYPE");
        f._env.unset_key("AIDA_AGENT_NAME");
        let approve =
            |summary: &str| as_operator(|| f.record_as("STORY-77", "approved", summary, &[], None));
        NEXT_REVIEW_FAULT.with(|c| c.set(Some(ReviewFault::Unsynced)));
        let error = format!("{:#}", approve("first pass").unwrap_err());
        assert!(error.contains("not confirmed durable"), "{error}");
        assert_eq!(NEXT_REVIEW_FAULT.with(|c| c.get()), None, "the fault fired");
        let named = event_receipts(&f);
        assert_eq!(named.len(), 1, "the event is on the ledger");
        assert_eq!(receipts(&f), named, "and its receipt was not removed");
        assert!(ra::human_receipt_for(&named[0], &f.head).is_some());
        assert_eq!(announced(&f), 0, "an unconfirmed act is not announced");
        let ledger = ledger_bytes(&f);

        // The retry's own sync fails: still unconfirmed, still refused.
        NEXT_REVIEW_FAULT.with(|c| c.set(Some(ReviewFault::Unsynced)));
        let error = format!("{:#}", approve("first pass").unwrap_err());
        assert!(error.contains("could not be synced"), "{error}");
        assert_eq!(
            NEXT_REVIEW_FAULT.with(|c| c.get()),
            None,
            "the retry synced"
        );
        assert_eq!(ledger_bytes(&f), ledger);
        assert_eq!(receipts(&f), named);
        assert_eq!(announced(&f), 0);

        approve("first pass").unwrap();
        assert_eq!(ledger_bytes(&f), ledger, "the retry appends nothing");
        assert_eq!(receipts(&f), named, "and issues no second receipt");
        assert_eq!(announced(&f), 1, "the confirmed act is announced");
        approve("first pass").unwrap();
        assert_eq!(announced(&f), 1, "once");
        // A later act leaves the kept receipt alone: a record names it.
        approve("second pass").unwrap();
        let two = event_receipts(&f);
        assert_eq!(two.len(), 2);
        assert_eq!(two[0], named[0]);
        assert_eq!(receipts(&f).len(), 2);
        assert!(ra::human_receipt_for(&named[0], &f.head).is_some());
        assert_eq!(reservations(&f), 0);
    }
    /// The event feed cannot be appended to while a review is recorded: the
    /// record stands, its announcement stays pending, and repeating the
    /// record announces it, once.
    // trace:BUG-2662 | ai:claude
    #[test]
    fn bug_2662_announcement_whose_emit_failed_stays_pending_for_the_retry() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let mut f = Fixture::new();
        f._env.unset_key("AIDA_AGENT_TYPE");
        f._env.unset_key("AIDA_AGENT_NAME");
        let approve =
            || as_operator(|| f.record_as("STORY-77", "approved", "first pass", &[], None));
        let marks = || -> Vec<serde_json::Value> {
            let store = crate::gate::ledger::store_root(&f.root).unwrap();
            std::fs::read_dir(store.join(".aida/review-announced"))
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                        .map(|e| serde_json::from_slice(&std::fs::read(e.path()).unwrap()).unwrap())
                        .collect()
                })
                .unwrap_or_default()
        };
        // A directory where the feed's file belongs: every append fails.
        let feed = crate::events::events_path(&f.root);
        let aside = feed.with_extension("aside");
        let had_feed = feed.exists();
        if had_feed {
            std::fs::rename(&feed, &aside).unwrap();
        }
        std::fs::create_dir_all(&feed).unwrap();
        approve().unwrap();
        std::fs::remove_dir(&feed).unwrap();
        if had_feed {
            std::fs::rename(&aside, &feed).unwrap();
        }
        assert_eq!(verdict_events(&f).len(), 1, "the review is on record");
        assert_eq!(announced(&f), 0);
        let pending = marks();
        assert_eq!(pending.len(), 1, "the announcement was claimed");
        assert!(pending[0].get("announced_at").is_none(), "{pending:?}");

        approve().unwrap();
        assert_eq!(verdict_events(&f).len(), 1);
        assert_eq!(announced(&f), 1, "the retry announces the pending act");
        let done = marks();
        assert_eq!(done.len(), 1);
        assert!(done[0].get("announced_at").is_some(), "{done:?}");
        approve().unwrap();
        assert_eq!(announced(&f), 1, "and only once");
    }
    /// The next act at a destination cannot remove the receipt a stopped
    /// recording of another act reserved there: it adopts that receipt's id
    /// rather than reserve a new one over it, so the slot never loses track
    /// of a receipt. One receipt exists, the one on record.
    // trace:BUG-2662 | ai:claude
    #[test]
    fn bug_2662_reserved_receipt_that_cannot_be_removed_is_adopted_not_orphaned() {
        use crate::gate::ledger::store::{ReviewFault, NEXT_REVIEW_FAULT};
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let mut f = Fixture::new();
        f._env.unset_key("AIDA_AGENT_TYPE");
        f._env.unset_key("AIDA_AGENT_NAME");
        let approve =
            |summary: &str| as_operator(|| f.record_as("STORY-77", "approved", summary, &[], None));
        NEXT_REVIEW_FAULT.with(|c| c.set(Some(ReviewFault::ExitAfterReceipt)));
        approve("first pass").unwrap_err();
        let orphan = receipts(&f);
        assert_eq!(orphan.len(), 1, "the receipt was persisted");
        assert_eq!(reservations(&f), 1);

        ra::FAIL_NEXT_RESERVED_RECEIPT_REMOVAL.with(|c| c.set(true));
        approve("another pass").unwrap();
        assert!(
            !ra::FAIL_NEXT_RESERVED_RECEIPT_REMOVAL.with(|c| c.get()),
            "the removal was attempted"
        );
        let named = event_receipts(&f);
        assert_eq!(named, orphan, "the reserved id was adopted");
        assert_eq!(receipts(&f), named, "no second receipt, no orphan");
        assert!(ra::human_receipt_for(&named[0], &f.head).is_some());
        assert_eq!(reservations(&f), 0);
    }
    /// The recorder exits after its receipt is persisted and before the
    /// ledger write, so nothing settles the receipt. The next recording at
    /// that destination recovers it: the same act adopts it, and any other
    /// act removes it and names its own. Either way one receipt exists, the
    /// one on record.
    // trace:BUG-2662 | ai:claude
    #[test]
    fn bug_2662_receipt_of_a_recorder_that_exited_is_recovered_by_the_next_attempt() {
        use crate::gate::ledger::store::{ReviewFault, NEXT_REVIEW_FAULT};
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        for same_act in [true, false] {
            let mut f = Fixture::new();
            f._env.unset_key("AIDA_AGENT_TYPE");
            f._env.unset_key("AIDA_AGENT_NAME");
            let approve = |summary: &str| {
                as_operator(|| f.record_as("STORY-77", "approved", summary, &[], None))
            };
            NEXT_REVIEW_FAULT.with(|c| c.set(Some(ReviewFault::ExitAfterReceipt)));
            let error = format!("{:#}", approve("first pass").unwrap_err());
            assert!(error.contains("injected recorder exit"), "{error}");
            assert!(verdict_events(&f).is_empty(), "the event was never written");
            let orphan = receipts(&f);
            assert_eq!(orphan.len(), 1, "the receipt was persisted");
            assert_eq!(reservations(&f), 1, "and is reserved for its attempt");

            approve(if same_act {
                "first pass"
            } else {
                "another pass"
            })
            .unwrap();
            let named = event_receipts(&f);
            assert_eq!(named.len(), 1);
            assert_eq!(
                receipts(&f),
                named,
                "same_act={same_act}: no orphan is left"
            );
            assert_eq!(named == orphan, same_act, "the same act adopts the receipt");
            assert!(ra::human_receipt_for(&named[0], &f.head).is_some());
            assert_eq!(reservations(&f), 0);
        }
    }
    /// A recorded refusal is announced once, whatever becomes of its hold:
    /// a repeat after the hold was released restores the hold and announces
    /// nothing, and a mixed refusal whose filing failed is announced by the
    /// retry that completes it even though an equal hold already stands.
    // trace:BUG-2662 | ai:claude
    #[test]
    fn bug_2662_refusal_is_announced_once_whatever_its_hold() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let f = Fixture::new();
        let config = f.root.join(".aida/config.toml");
        let original = std::fs::read_to_string(&config).unwrap_or_default();
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            format!("{original}\n[review]\nblocking_tiers = [\"p1\"]\n"),
        )
        .unwrap();
        crate::review_blocking::commit_config_for_test(&f.root);
        let findings = [
            serde_json::json!({"text":"blocking defect","severity":"major","blocking_tier":"p1"})
                .to_string(),
            crate::review_blocking::tests::p2_finding(),
        ];
        let reviewer = f.reviewer().unwrap();
        let refuse = |summary: &str| {
            f.record_as(
                "STORY-77",
                "request-changes",
                summary,
                &findings,
                Some(&reviewer),
            )
        };
        let hold_events = |f: &Fixture| {
            ledger_events(f)
                .iter()
                .filter(|e| matches!(e.body, crate::gate::ledger::EventBody::Hold { .. }))
                .count()
        };
        refuse("first refusal").unwrap();
        assert_eq!(announced(&f), 1);

        // Announced, then its hold released: the repeat restores the hold
        // and does not announce the refusal a second time.
        let hold = crate::gate::merge_hold_core::read_hold_record(&f.root, 59).unwrap();
        crate::gate::merge_hold_core::release_hold_by(&f.root, &hold, "operator", None).unwrap();
        assert!(crate::gate::merge_hold_core::read_hold_record(&f.root, 59).is_none());
        refuse("first refusal").unwrap();
        assert!(crate::gate::merge_hold_core::read_hold_record(&f.root, 59).is_some());
        assert_eq!(verdict_events(&f).len(), 1);
        assert_eq!(
            announced(&f),
            1,
            "an announced refusal is not announced again"
        );

        // A second refusal whose follow-up filing fails after it is on
        // record: nothing announced yet. The retry finds an equal hold
        // standing, places none, and still announces the refusal, once.
        crate::gate::ledger::FAIL_NEXT_FINDING_DELIVERY.with(|c| c.set(true));
        let error = refuse("second refusal").unwrap_err().to_string();
        assert!(error.contains("deferred findings pending"), "{error}");
        assert_eq!(verdict_events(&f).len(), 2);
        assert_eq!(announced(&f), 1, "the failed recording announced nothing");
        let holds = hold_events(&f);
        refuse("second refusal").unwrap();
        assert_eq!(verdict_events(&f).len(), 2);
        assert_eq!(hold_events(&f), holds, "the standing hold was enough");
        assert_eq!(announced(&f), 2, "the completed recording is announced");
        refuse("second refusal").unwrap();
        assert_eq!(announced(&f), 2, "and only once");
    }
    /// Two identical human-at-TTY approvals at once: the second lands after
    /// the first made its reads and before it takes the ledger locks. The
    /// receipt is chosen under those locks, so the later one answers with the
    /// receipt already on record: one event, one receipt.
    // trace:BUG-2662 | ai:claude
    #[test]
    fn bug_2662_simultaneous_operator_approvals_share_one_receipt() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let mut f = Fixture::new();
        f._env.unset_key("AIDA_AGENT_TYPE");
        f._env.unset_key("AIDA_AGENT_NAME");
        let (root, head) = (f.root.clone(), f.head.clone());
        let _race = race_next_append(move || {
            crate::handle_review_record_context_at(
                root.clone(),
                "STORY-77",
                "approved",
                Some(&head),
                Some("feature"),
                Some("first pass"),
                &[],
                &[],
                Some(59),
                false,
                |_, _| Ok(head.clone()),
                None,
            )
            .unwrap();
        });
        as_operator(|| f.record_as("STORY-77", "approved", "first pass", &[], None)).unwrap();
        let named = event_receipts(&f);
        assert_eq!(named.len(), 1, "one verdict event for both");
        assert_eq!(receipts(&f), named, "and one receipt, the one it names");
        assert_eq!(announced(&f), 1, "the later repeat announces nothing");
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
            let launch = LaunchDraft::prepare(&f.root, 59, f.head.clone()).unwrap();
            let body = serde_json::json!({"version":1,"pr":59,"reviewed_sha":f.head,
                "verdict":"request-changes","summary":"complete child rationale",
                "findings":[policy::p2_finding()]});
            std::fs::write(&launch.path, body.to_string()).unwrap();
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
