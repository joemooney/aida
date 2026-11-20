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
    /// Today's behaviour once the launcher that owns a draft is gone: the
    /// draft is never recorded by a later launcher. The first launch's
    /// reviewer grant stays bound to the dead launcher process, a restarted
    /// launcher cannot present it, and its own launch reads only its own
    /// (empty) draft directory. Nothing reaches the ledger; the draft stays
    /// on disk, unconsumed, and keeps its checkout from being removed.
    /// docs/plans/2026-11-20-bug-2669-retained-draft-recovery.md proposes the
    /// authorized recovery that replaces this outcome.
    // trace:BUG-2669 | ai:claude
    #[test]
    fn bug_2669_retained_draft_after_launcher_restart_is_not_recorded_and_stays_retained() {
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let mut f = Fixture::new();
        let first = LaunchDraft::prepare(&f.root, 59, f.head.clone()).unwrap();
        let body = serde_json::json!({"version":1,"pr":59,"reviewed_sha":f.head,"verdict":"approved",
            "summary":"complete child rationale","findings":[]});
        std::fs::write(&first.path, body.to_string()).unwrap();
        let retained = first.path.clone();
        // The launcher exits before recording: its descriptor is gone and the
        // reviewer grant remains bound to a process that no longer exists.
        drop(first);
        sa::test_support::rewrite_grant(&f.child, |g| {
            g.binding = vec!["proc:4194399@1".into()];
        });
        assert!(
            f.reviewer().is_err(),
            "a grant bound to the dead launcher must not be presentable"
        );

        // A restarted launcher holds its own grant and issues its own child.
        let restarted = sa::test_support::mint_grant_for(
            &f.root,
            &crate::current_user_id(None),
            "advisor",
            &["reviewer"],
        );
        f._env.set_key(sa::GRANT_ENV, &restarted);
        let since = SystemTime::now();
        let child = crate::launch::issue_launch_child(
            &f.root,
            "reviewer",
            &crate::current_user_id(None),
            false,
        )
        .unwrap()
        .id;
        assert!(
            VerifiedReviewer::from_launch(
                &f.root,
                &f.child,
                &restarted,
                f.since,
                "independent-review-session"
            )
            .is_err(),
            "a later launcher must not record under the first launch's grant"
        );
        // Its own launch is sound, but sees no draft: the launcher treats
        // this NotFound as "no handoff" and never adopts the earlier one.
        assert!(VerifiedReviewer::from_launch(
            &f.root,
            &child,
            &restarted,
            since,
            "second-review-session"
        )
        .is_ok());
        let second = LaunchDraft::prepare(&f.root, 59, f.head.clone()).unwrap();
        let missing = second.read().unwrap_err();
        assert!(
            missing
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound),
            "{missing:#}"
        );

        assert!(rv::read_recorded_verdict(&f.root, "PR-59").is_none());
        assert_eq!(unconsumed_drafts(&f.root).unwrap(), vec![retained]);
        assert!(unconsumed_draft_retention(&f.root)
            .unwrap()
            .contains("unrecorded reviewer draft"));
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
