//! `aida deputy`: a stand-in for the operator that runs jobs other sessions
//! submit, under the grant held by the operator's own interactive shell.
//!
//! The spool lives in the main checkout at `.aida/deputy/<name>/` (0700, owner
//! checked at every scan). A submit writes one `incoming/<id>.job` atomically;
//! the worker claims it by renaming it into `running/`, so two submits never
//! clobber each other and a job runs once. Each job runs as a child process,
//! never sourced into the loop. Its output goes to `out/<id>.out`, its result
//! to `done|failed/<id>.rc.json`, and every step to the append-only
//! `deputy.ledger`.
//!
//! The central rule: a job gets a terminal only when its argv matches a live
//! entry in the operator-authored TTY policy. Every other job runs with no
//! controlling terminal, so each human-TTY gate refuses it as it would refuse
//! an agent.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude

pub(crate) mod policy;
#[cfg(unix)]
pub(crate) mod worker;

#[cfg(test)]
mod tests;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Environment a job runs under: its id and the spool it came from. The
/// integrity floor reads both (see [`job_floor_authorized`]).
pub(crate) const JOB_ENV: &str = "AIDA_DEPUTY_JOB";
pub(crate) const SPOOL_ENV: &str = "AIDA_DEPUTY_SPOOL";
pub(crate) const REQUESTER_ENV: &str = "AIDA_DEPUTY_REQUESTER";
/// Set as a per-command prefix by the `aida()` shell function only, so a bare
/// binary run (no function) is told to start the deputy from the shell.
pub(crate) const VIA_WRAPPER_ENV: &str = "AIDA_DEPUTY_VIA_WRAPPER";

pub(crate) const DEFAULT_NAME: &str = "default";
pub(crate) const STATES: [&str; 4] = ["incoming", "running", "done", "failed"];

/// The flags that bypass a gate. A job carrying one never gets a terminal.
pub(crate) const BYPASS_FLAGS: [&str; 4] = ["--force", "--no-verify", "--admin", "--yes-i-know"];

/// One deputy's spool directory.
#[derive(Debug, Clone)]
pub(crate) struct Spool {
    pub root: PathBuf,
}

/// Who submitted a job. Audit data, not authority: a same-uid process can
/// write any value here (slice 3 adds a submit capability).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct Requester {
    pub session_id: Option<String>,
    pub role: Option<String>,
    pub seat: Option<String>,
    pub subject: Option<String>,
    /// First 8 characters of the submitter's grant id, never the full handle.
    pub grant_prefix: Option<String>,
    /// Whether `submit` validated the submitter's own grant.
    pub grant_verified: bool,
    pub spec: Option<String>,
    pub pid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JobBody {
    /// Exec'd directly, no shell. The only form that may get a terminal.
    Argv(Vec<String>),
    /// Run by `bash --noprofile --norc -c`; needs `--allow-scripts`.
    Script(String),
}

impl JobBody {
    pub(crate) fn digest(&self) -> String {
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        format!("{:x}", Sha256::digest(&bytes))
    }

    pub(crate) fn summary(&self) -> String {
        match self {
            Self::Argv(argv) => argv.join(" "),
            Self::Script(body) => {
                let first = body.lines().next().unwrap_or("").trim();
                format!("script: {first}")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct JobRecord {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub requester: Requester,
    pub serial: Option<String>,
    pub timeout_secs: u64,
    pub body: JobBody,
    pub sha256: String,
}

/// A finished job's result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct JobResult {
    pub id: String,
    pub rc: i32,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: DateTime<Utc>,
    /// `ok`, `exit`, `timeout`, `needs-operator`, `cancelled`, `orphaned`,
    /// or `refused:<why>`.
    pub reason: String,
    pub tty: bool,
    pub policy_entry: Option<String>,
    pub output: PathBuf,
}

/// rc for a timed-out job (as `timeout(1)`).
pub(crate) const RC_TIMEOUT: i32 = 124;
/// rc for a terminal job that sat waiting on a prompt.
pub(crate) const RC_NEEDS_OPERATOR: i32 = 125;
/// rc for a job refused before it ran.
pub(crate) const RC_REFUSED: i32 = 126;
/// rc for a cancelled or orphaned job.
pub(crate) const RC_CANCELLED: i32 = 130;

/// The spool root for deputy `name` under the main checkout.
pub(crate) fn spool_for(main_root: &Path, name: &str) -> Result<Spool> {
    validate_name(name)?;
    Ok(Spool {
        root: main_root.join(".aida").join("deputy").join(name),
    })
}

/// The spool for `name` in the current project's main checkout.
pub(crate) fn current_spool(name: &str) -> Result<Spool> {
    let main = crate::worktree_gc::find_main_worktree_root()
        .context("aida deputy needs a git checkout")?;
    spool_for(&main, name)
}

fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        bail!("deputy name must be 1-40 letters, digits, '-' or '_' (got {name:?})");
    }
    Ok(())
}

/// Job ids are UUIDv7, so they sort by submit time. Anything else is refused,
/// which also keeps ids path-safe.
pub(crate) fn validate_id(id: &str) -> Result<()> {
    match uuid::Uuid::parse_str(id) {
        Ok(u) if u.hyphenated().to_string() == id => Ok(()),
        _ => bail!("not a deputy job id: {id:?}"),
    }
}

impl Spool {
    pub(crate) fn dir(&self, state: &str) -> PathBuf {
        self.root.join(state)
    }
    pub(crate) fn out_dir(&self) -> PathBuf {
        self.root.join("out")
    }
    pub(crate) fn ledger_path(&self) -> PathBuf {
        self.root.join("deputy.ledger")
    }
    pub(crate) fn policy_path(&self) -> PathBuf {
        self.root.join("deputy.policy")
    }
    pub(crate) fn lock_path(&self) -> PathBuf {
        self.root.join(".deputy.lock")
    }
    pub(crate) fn job_path(&self, state: &str, id: &str) -> PathBuf {
        self.dir(state).join(format!("{id}.job"))
    }
    pub(crate) fn result_path(&self, state: &str, id: &str) -> PathBuf {
        self.dir(state).join(format!("{id}.rc.json"))
    }
    pub(crate) fn out_path(&self, id: &str) -> PathBuf {
        self.out_dir().join(format!("{id}.out"))
    }
    /// Present while a job runs on a terminal the policy authorized.
    pub(crate) fn tty_marker_path(&self, id: &str) -> PathBuf {
        self.dir("running").join(format!("{id}.tty"))
    }
    pub(crate) fn cancel_marker_path(&self, id: &str) -> PathBuf {
        self.dir("running").join(format!("{id}.cancel"))
    }

    /// Create the spool tree, 0700, and check it.
    pub(crate) fn ensure(&self) -> Result<()> {
        for d in [
            self.root.clone(),
            self.dir("incoming"),
            self.dir("running"),
            self.dir("done"),
            self.dir("failed"),
            self.out_dir(),
        ] {
            create_private_dir(&d)?;
        }
        self.check_owned()
    }

    /// Refuse a spool directory not owned by us or open to group/others.
    pub(crate) fn check_owned(&self) -> Result<()> {
        for d in [
            self.root.clone(),
            self.dir("incoming"),
            self.dir("running"),
            self.dir("done"),
            self.dir("failed"),
            self.out_dir(),
        ] {
            check_private_dir(&d)?;
        }
        Ok(())
    }

    /// Write `rec` as `incoming/<id>.job`: 0600, `create_new` on a dot-tmp,
    /// then a rename, so a half-written job is never visible and an existing
    /// job is never overwritten.
    pub(crate) fn submit(&self, rec: &JobRecord) -> Result<PathBuf> {
        validate_id(&rec.id)?;
        let incoming = self.dir("incoming");
        if !incoming.is_dir() {
            bail!(
                "no deputy spool at {} — the operator starts one with `aida deputy` in their terminal",
                self.root.display()
            );
        }
        check_private_dir(&self.root)?;
        check_private_dir(&incoming)?;
        let tmp = incoming.join(format!(".{}.tmp", rec.id));
        let dest = self.job_path("incoming", &rec.id);
        write_private_new(&tmp, &serde_json::to_vec_pretty(rec)?)?;
        if dest.exists() {
            let _ = std::fs::remove_file(&tmp);
            bail!("job {} already exists", rec.id);
        }
        std::fs::rename(&tmp, &dest).context("publish job")?;
        Ok(dest)
    }

    /// Where job `id` is now, with its record.
    pub(crate) fn locate(&self, id: &str) -> Result<Option<(&'static str, PathBuf)>> {
        validate_id(id)?;
        for state in STATES {
            let p = self.job_path(state, id);
            if p.exists() {
                return Ok(Some((state, p)));
            }
        }
        Ok(None)
    }

    pub(crate) fn read_result(&self, id: &str) -> Option<JobResult> {
        for state in ["done", "failed"] {
            if let Ok(bytes) = std::fs::read(self.result_path(state, id)) {
                if let Ok(r) = serde_json::from_slice(&bytes) {
                    return Some(r);
                }
            }
        }
        None
    }

    /// `incoming/*.job` ids, oldest first (UUIDv7 sorts by time).
    pub(crate) fn list_ids(&self, state: &str) -> Vec<String> {
        let mut ids: Vec<String> = std::fs::read_dir(self.dir(state))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let id = name.strip_suffix(".job")?;
                validate_id(id).ok()?;
                Some(id.to_string())
            })
            .collect();
        ids.sort();
        ids
    }

    /// Append one ledger row in a single `O_APPEND` write.
    pub(crate) fn ledger(&self, row: &LedgerRow) -> Result<()> {
        let mut line = serde_json::to_vec(row)?;
        line.push(b'\n');
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(self.ledger_path())
            .context("open deputy ledger")?;
        f.write_all(&line).context("append deputy ledger")?;
        Ok(())
    }

    /// Write a result atomically into `state` (`done` or `failed`).
    pub(crate) fn write_result(&self, state: &str, res: &JobResult) -> Result<()> {
        let tmp = self.dir(state).join(format!(".{}.rc.tmp", res.id));
        let _ = std::fs::remove_file(&tmp);
        write_private_new(&tmp, &serde_json::to_vec_pretty(res)?)?;
        std::fs::rename(&tmp, self.result_path(state, &res.id))?;
        Ok(())
    }
}

/// One `deputy.ledger` row.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct LedgerRow {
    pub ts: DateTime<Utc>,
    pub job: String,
    /// `submitted|claimed|refused|started|finished|cancelled|orphaned|policy`.
    pub event: String,
    pub deputy: String,
    pub requester: Option<Requester>,
    pub summary: Option<String>,
    pub sha256: Option<String>,
    pub worker: Option<usize>,
    pub tty: Option<bool>,
    pub policy_entry: Option<String>,
    pub rc: Option<i32>,
    pub reason: Option<String>,
    pub output: Option<PathBuf>,
}

impl LedgerRow {
    pub(crate) fn new(deputy: &str, job: &str, event: &str) -> Self {
        Self {
            ts: Utc::now(),
            job: job.to_string(),
            event: event.to_string(),
            deputy: deputy.to_string(),
            ..Default::default()
        }
    }
}

pub(crate) fn create_private_dir(d: &Path) -> Result<()> {
    if !d.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(d)
                .with_context(|| format!("create {}", d.display()))?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(d)?;
    }
    Ok(())
}

/// A spool directory must be a real directory (not a symlink), owned by us,
/// with no group/other bits.
pub(crate) fn check_private_dir(d: &Path) -> Result<()> {
    let md = std::fs::symlink_metadata(d).with_context(|| format!("stat {}", d.display()))?;
    if !md.file_type().is_dir() {
        bail!("{} is not a directory", d.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if md.uid() != current_uid() {
            bail!("{} is not owned by you; refusing it", d.display());
        }
        if md.mode() & 0o077 != 0 {
            bail!(
                "{} is open to group/other (mode {:o}); it must be 0700",
                d.display(),
                md.mode() & 0o777
            );
        }
    }
    Ok(())
}

fn write_private_new(p: &Path, bytes: &[u8]) -> Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(p)
        .with_context(|| format!("create {}", p.display()))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// Why a job file is refused before it is parsed as authority (AC2).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JobFileRefusal {
    NotRegular,
    ForeignOwner,
    Writable,
    Linked,
    Unreadable(String),
    Malformed(String),
    IdMismatch,
}

impl JobFileRefusal {
    pub(crate) fn code(&self) -> String {
        match self {
            Self::NotRegular => "refused:not-a-regular-file".into(),
            Self::ForeignOwner => "refused:foreign-owner".into(),
            Self::Writable => "refused:group-or-world-writable".into(),
            Self::Linked => "refused:hard-linked".into(),
            Self::Unreadable(e) => format!("refused:unreadable ({e})"),
            Self::Malformed(e) => format!("refused:malformed ({e})"),
            Self::IdMismatch => "refused:id-mismatch".into(),
        }
    }
}

/// Inspect a job file without following links, then parse it.
pub(crate) fn read_job_file(path: &Path, id: &str) -> Result<JobRecord, JobFileRefusal> {
    let md =
        std::fs::symlink_metadata(path).map_err(|e| JobFileRefusal::Unreadable(e.to_string()))?;
    if !md.file_type().is_file() {
        return Err(JobFileRefusal::NotRegular);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if md.uid() != current_uid() {
            return Err(JobFileRefusal::ForeignOwner);
        }
        if md.mode() & 0o022 != 0 {
            return Err(JobFileRefusal::Writable);
        }
        if md.nlink() > 1 {
            return Err(JobFileRefusal::Linked);
        }
    }
    let bytes = std::fs::read(path).map_err(|e| JobFileRefusal::Unreadable(e.to_string()))?;
    let rec: JobRecord =
        serde_json::from_slice(&bytes).map_err(|e| JobFileRefusal::Malformed(e.to_string()))?;
    if rec.id != id || rec.sha256 != rec.body.digest() {
        return Err(JobFileRefusal::IdMismatch);
    }
    Ok(rec)
}

/// The submitter's identity, as this process can see it. The grant is
/// validated with the existing presenter check, so `grant_verified` is only
/// true when this process actually holds the grant it names.
pub(crate) fn current_requester(spec: Option<String>) -> Requester {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let mut r = Requester {
        session_id: env("AIDA_SESSION_ID"),
        role: env("AIDA_SESSION_ROLE"),
        spec,
        pid: std::process::id(),
        ..Default::default()
    };
    if let Ok(main) = crate::worktree_gc::find_main_worktree_root() {
        if let crate::gate::seat_authority::GrantCheck::Valid(g) =
            crate::gate::seat_authority::grant_check(&main)
        {
            r.seat = Some(g.seat.clone());
            r.subject = Some(g.subject.clone());
            r.grant_prefix = Some(g.id.chars().take(8).collect());
            r.grant_verified = true;
        }
    }
    r
}

/// The marker a terminal job's floor check reads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct TtyMarker {
    pub job: String,
    pub entry: String,
    pub expires_at: DateTime<Utc>,
}

/// Q6 defence in depth for the integrity floor: under a deputy job
/// (`AIDA_DEPUTY_JOB` set), the floor holds only while the deputy's own marker
/// says the policy authorized this job a terminal. A job that finds a terminal
/// some other way (`script(1)`, a pty helper) is refused. Outside a deputy job
/// this is always true.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
pub(crate) fn job_floor_authorized() -> bool {
    let job = std::env::var(JOB_ENV).ok();
    let spool = std::env::var(SPOOL_ENV).ok();
    job_floor_authorized_from(job.as_deref(), spool.as_deref(), Utc::now())
}

pub(crate) fn job_floor_authorized_from(
    job: Option<&str>,
    spool: Option<&str>,
    now: DateTime<Utc>,
) -> bool {
    let Some(job) = job.filter(|j| !j.is_empty()) else {
        return true;
    };
    let Some(spool) = spool.filter(|s| !s.is_empty()) else {
        return false;
    };
    if validate_id(job).is_err() {
        return false;
    }
    let spool = Spool {
        root: PathBuf::from(spool),
    };
    if check_private_dir(&spool.root).is_err() || check_private_dir(&spool.dir("running")).is_err()
    {
        return false;
    }
    let marker = spool.tty_marker_path(job);
    let Ok(md) = std::fs::symlink_metadata(&marker) else {
        return false;
    };
    if !md.file_type().is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if md.uid() != current_uid() || md.mode() & 0o022 != 0 {
            return false;
        }
    }
    let Ok(bytes) = std::fs::read(&marker) else {
        return false;
    };
    match serde_json::from_slice::<TtyMarker>(&bytes) {
        Ok(m) => m.job == job && m.expires_at > now,
        Err(_) => false,
    }
}

pub(crate) fn write_tty_marker(spool: &Spool, marker: &TtyMarker) -> Result<()> {
    let p = spool.tty_marker_path(&marker.job);
    let _ = std::fs::remove_file(&p);
    write_private_new(&p, &serde_json::to_vec(marker)?)
}

pub(crate) fn new_job_id() -> String {
    uuid::Uuid::now_v7().hyphenated().to_string()
}
