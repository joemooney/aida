//! Outbound-only work relay and home-only status publisher. No imported commands.
// trace:STORY-2480 | ai:codex
use crate::ci_receipt::{self, Inventory, Job, Receipt, Status};
use crate::intent_intake::{self, Intake};
use aida_core::transport_policy::Private;
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

const CONFIG: &str = "ci-profile.json";
const CONTEXT: &str = "aida/work-ci";
const REPO_CONFIG: &[u8] = b"[core]\n\trepositoryformatversion = 0\n\tbare = true\n";
#[derive(Debug, Subcommand)]
pub(crate) enum CiCommand {
    /// Enroll a reviewed clone-local CI profile and probe return-credential restrictions.
    Enroll {
        /// Strict profile JSON; copies endpoint policy and credential variable names.
        #[clap(long)]
        file: PathBuf,
        /// Confirm destination and server-side credential policy review.
        #[clap(long, required = true)]
        reviewed: bool,
    },
    /// Home only: create retained immutable home/work refs for an explicit attempt.
    Request {
        /// Previously unused home-assigned positive u32; retained forever.
        #[clap(long)]
        attempt: u32,
        /// Exact full lowercase commit object ID already available at home.
        #[clap(long)]
        sha: String,
    },
    /// Run one bounded work polling pass; invoke periodically from a user timer.
    Poll,
    /// Home only: validate intake and publish native statuses for latest attempts.
    Process,
    /// Audit retained results, or revalidate one exact attempt/SHA before reading it.
    Read {
        /// Exact retained attempt to revalidate; requires --sha.
        #[clap(long, requires = "sha")]
        attempt: Option<u32>,
        /// Exact retained source SHA; requires --attempt.
        #[clap(long, requires = "attempt")]
        sha: Option<String>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Endpoint {
    url: String,
    project: String,
    token_env: String,
    forge: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Profile {
    version: u32,
    role: String,
    home_remote: String,
    work_remote: String,
    work_api: Endpoint,
    home_api: Option<Endpoint>,
    timeout_seconds: u32,
    max_polls: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    sha: String,
    attempt: u32,
    deadline: i64,
    polls: u32,
    pipeline: Option<u64>,
    receipt_parts: usize,
    done: bool,
}
fn git(root: &Path, args: &[&str]) -> Result<String> {
    // Remote diagnostics are local secrets, never forwarded to home or stdout.
    intent_intake::git(root, args, &[])
        .and_then(|b| String::from_utf8(b).map_err(Into::into))
        .map(|s| s.trim().to_owned())
        .map_err(|_| anyhow::anyhow!("CI Git operation refused; inspect local configuration"))
}
fn endpoint(e: &Endpoint) -> Result<()> {
    let url = reqwest::Url::parse(&e.url).map_err(|_| anyhow::anyhow!("Invalid CI API URL"))?;
    if !(url.scheme() == "https"
        || (url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(e.forge.as_str(), "gitlab" | "github")
        || e.token_env.is_empty()
        || !e
            .token_env
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
    {
        bail!("Invalid CI API endpoint or credential environment name");
    }
    if e.forge == "gitlab" {
        if e.project.parse::<u64>().ok().filter(|n| *n > 0).is_none() {
            bail!("GitLab project must be a positive numeric ID");
        }
    } else {
        let parts: Vec<_> = e.project.split('/').collect();
        if parts.len() != 2
            || parts.iter().any(|s| {
                s.is_empty()
                    || *s == "."
                    || *s == ".."
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            })
        {
            bail!("GitHub project must be owner/repository");
        }
    }
    Ok(())
}
fn profile(intake: &Intake) -> Result<Profile> {
    let p: Profile = serde_json::from_slice(
        &intake
            .private
            .read(CONFIG)?
            .context("Intake contains CI results but this clone has no CI admission profile; enroll with aida intent ci enroll --file <profile.json> --reviewed before reading shared CI history")?,
    )
    .map_err(|_| anyhow::anyhow!("Invalid CI profile"))?;
    validate_profile(&p)?;
    Ok(p)
}
fn validate_profile(p: &Profile) -> Result<()> {
    if p.version != 1
        || !matches!(p.role.as_str(), "home" | "work")
        || !(60..=86400).contains(&p.timeout_seconds)
        || !(1..=4096).contains(&p.max_polls)
        || p.work_api.forge != "gitlab"
        || (p.role == "home") != p.home_api.is_some()
    {
        bail!("Invalid CI profile role, version or bounds");
    }
    intent_intake::validate_remote(&p.home_remote)?;
    intent_intake::validate_remote(&p.work_remote)?;
    endpoint(&p.work_api)?;
    if let Some(e) = &p.home_api {
        endpoint(e)?;
    }
    Ok(())
}
fn source(intake: &Intake) -> Result<PathBuf> {
    let root = intake.worktree.parent().unwrap().join("ci-source");
    if !root.exists() {
        fs::DirBuilder::new().mode(0o700).create(&root)?;
        git(&root, &["init", "--bare", "--quiet", "--template="])?;
        // Git creates this file using the caller's umask (0644 in CI).
        // Normalize only our freshly initialized private repository, never
        // repair a pre-existing configuration whose identity is untrusted.
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.join("config"), fs::Permissions::from_mode(0o600))?;
        Private::open(&root)?.write("config", REPO_CONFIG)?;
    }
    let private = Private::open(&root)?;
    if private.read("config")?.as_deref() != Some(REPO_CONFIG) {
        bail!("CI source Git config changed");
    }
    for forbidden in [
        "commondir",
        "objects/info/alternates",
        "shallow",
        "info/grafts",
    ] {
        if fs::symlink_metadata(root.join(forbidden)).is_ok() {
            bail!("CI requires an independent complete object database");
        }
    }
    for dir in ["objects", "refs"] {
        Private::open(&root.join(dir))?;
    }
    Ok(root)
}
fn refs(root: &Path, remote: &str) -> Result<BTreeMap<u32, String>> {
    let mut result = BTreeMap::new();
    // remote-creds-ok(TASK-2001): isolated intake uses reviewed explicit URL and SSH identity; ambient credential helpers are deliberately disabled.
    let output = git(
        root,
        &["ls-remote", "--refs", "--", remote, "refs/aida/ci/*"],
    )?;
    if output.lines().count() > 4096 {
        bail!("Too many retained CI requests");
    }
    for line in output.lines() {
        let (sha, name) = line
            .split_once('\t')
            .context("Invalid CI request listing")?;
        let n = ci_receipt::attempt(
            name.strip_prefix("refs/aida/ci/")
                .context("Invalid CI request ref")?,
        )?;
        if !ci_receipt::oid(sha) || result.insert(n, sha.into()).is_some() {
            bail!("Invalid or duplicate CI request identity");
        }
    }
    Ok(result)
}
fn fetch(root: &Path, remote: &str, sha: &str) -> Result<()> {
    // remote-creds-ok(TASK-2001): isolated intake explicit URL uses SSH identity, never host credential helpers.
    git(
        root,
        &[
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            "--no-recurse-submodules",
            "--",
            remote,
            sha,
        ],
    )?;
    if git(root, &["cat-file", "-t", sha])? != "commit" {
        bail!("CI request must directly target a commit");
    }
    Ok(())
}
fn immutable_push(root: &Path, remote: &str, sha: &str, target: &str) -> Result<()> {
    // remote-creds-ok(TASK-2001): isolated intake explicit URL uses SSH identity, never host credential helpers.
    let output = git(root, &["ls-remote", "--refs", "--", remote, target])?;
    if !output.is_empty() {
        if output != format!("{sha}\t{target}") {
            bail!("Immutable CI ref already has another target");
        }
        return Ok(());
    }
    // No force, deletion, tags or ambient push mappings. Server-side immutable
    // request rules remain mandatory to close the lookup/push race.
    // remote-creds-ok(TASK-2001): isolated intake explicit URL uses SSH identity, never host credential helpers.
    git(
        root,
        &[
            "push",
            "--no-verify",
            "--no-follow-tags",
            "--",
            remote,
            &format!("{sha}:{target}"),
        ],
    )?;
    Ok(())
}
use crate::ci_inventory::inventory;

pub(super) struct Snapshot {
    pub requests: BTreeMap<u32, String>,
    inventories: RefCell<BTreeMap<String, Inventory>>,
    root: PathBuf,
    remote: String,
    deadline: std::time::Instant,
}
impl Snapshot {
    pub fn load(intake: &Intake) -> Result<Self> {
        let p = profile(intake)?;
        let root = source(intake)?;
        let requests = refs(&root, &p.home_remote)?;
        Ok(Self {
            requests,
            inventories: RefCell::new(BTreeMap::new()),
            root,
            remote: p.home_remote,
            deadline: std::time::Instant::now() + Duration::from_secs(300),
        })
    }
    pub fn admit<'a>(&self, path: &'a str) -> Result<(&'a str, u32)> {
        let rest = path
            .strip_prefix("ci-results/")
            .context("Invalid CI path")?;
        let (sha, name) = rest.split_once('/').context("Invalid CI path")?;
        let n = ci_receipt::attempt(name.strip_suffix(".json").context("Invalid CI filename")?)?;
        if self.requests.get(&n).map(String::as_str) != Some(sha) {
            bail!("CI result has no retained home admission");
        }
        Ok((sha, n))
    }
    pub fn validate(&self, path: &str, bytes: &[u8]) -> Result<Receipt> {
        let (sha, n) = self.admit(path)?;
        if !self.inventories.borrow().contains_key(sha) {
            if std::time::Instant::now() >= self.deadline {
                bail!("CI admission validation deadline exceeded");
            }
            fetch(&self.root, &self.remote, sha)?;
            let inventory = inventory(&self.root, sha)?;
            if std::time::Instant::now() >= self.deadline {
                bail!("CI admission validation deadline exceeded");
            }
            self.inventories.borrow_mut().insert(sha.into(), inventory);
        }
        let receipt = Receipt::parse(
            bytes,
            self.inventories
                .borrow()
                .get(sha)
                .context("Missing pinned CI inventory")?,
            sha,
            n,
        )?;
        if receipt.path() != path {
            bail!("Noncanonical CI result path");
        }
        Ok(receipt)
    }
}

// API bodies, credentials, logs and remote error text never enter diagnostics.
fn api(e: &Endpoint, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
    let e = e.clone();
    let method = method.to_owned();
    let path = path.to_owned();
    std::thread::spawn(move || -> Result<Value> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(async move {
            let token = std::env::var(&e.token_env)
                .map_err(|_| anyhow::anyhow!("CI API credential unavailable"))?;
            if token.is_empty() {
                bail!("CI API credential unavailable");
            }
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .build()?;
            let mut request = client
                .request(
                    method.parse()?,
                    format!("{}{}", e.url.trim_end_matches('/'), path),
                )
                .header("User-Agent", "aida-ci-relay");
            request = if e.forge == "gitlab" {
                request.header("PRIVATE-TOKEN", token)
            } else {
                request.bearer_auth(token)
            };
            if let Some(body) = body {
                request = request.json(&body);
            }
            let mut response = request
                .send()
                .await
                .map_err(|_| anyhow::anyhow!("CI API request unavailable"))?;
            if !response.status().is_success() {
                bail!("CI API refused request");
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| anyhow::anyhow!("CI API read failed"))?
            {
                if bytes.len() + chunk.len() > 1024 * 1024 {
                    bail!("CI API response exceeds limit");
                }
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("Invalid CI API JSON"))
        })
    })
    .join()
    .map_err(|_| anyhow::anyhow!("CI API worker failed"))?
}
fn save(intake: &Intake, j: &Journal) -> Result<()> {
    intake.private.write(
        &format!("ci-attempt-{}.json", j.attempt),
        &serde_json::to_vec(j)?,
    )
}
fn journal(intake: &Intake, p: &Profile, n: u32, sha: &str) -> Result<Journal> {
    let name = format!("ci-attempt-{n}.json");
    if let Some(bytes) = intake.private.read(&name)? {
        let j: Journal = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid CI retry journal"))?;
        if j.version != 1
            || j.sha != sha
            || j.attempt != n
            || j.receipt_parts > 4
            || j.polls > p.max_polls
        {
            bail!("CI request or retry journal changed; manual recovery required");
        }
        return Ok(j);
    }
    let j = Journal {
        version: 1,
        sha: sha.into(),
        attempt: n,
        deadline: chrono::Utc::now().timestamp() + i64::from(p.timeout_seconds),
        polls: 0,
        pipeline: None,
        receipt_parts: 0,
        done: false,
    };
    save(intake, &j)?; // Before mirroring, API calls, or receipt publication.
    Ok(j)
}
fn terminal(value: &Value) -> Option<Status> {
    match value.as_str()? {
        "success" => Some(Status::Success),
        "failed" => Some(Status::Failed),
        "canceled" => Some(Status::Canceled),
        "skipped" => Some(Status::Skipped),
        _ => None,
    }
}
fn pipeline_receipt(p: &Profile, j: &Journal, inventory: &Inventory) -> Result<Option<Vec<u8>>> {
    let id = j.pipeline.context("Pipeline association missing")?;
    let prefix = format!("/projects/{}/pipelines/{id}", p.work_api.project);
    let pipeline = api(&p.work_api, "GET", &prefix, None)?;
    if pipeline["id"].as_u64() != Some(id)
        || pipeline["sha"].as_str() != Some(&j.sha)
        || pipeline["ref"].as_str() != Some(format!("ci/{}", j.attempt).as_str())
    {
        bail!("Pipeline identity mismatch");
    }
    let Some(status) = terminal(&pipeline["status"]) else {
        return Ok(None);
    };
    let mut jobs = Vec::new();
    let mut complete = false;
    for page in 1..=3 {
        let v = api(
            &p.work_api,
            "GET",
            &format!("{prefix}/jobs?include_retried=false&per_page=100&page={page}"),
            None,
        )?;
        let rows = v.as_array().context("Invalid CI jobs response")?;
        for row in rows {
            let name = row["name"].as_str().context("CI job name missing")?;
            if !inventory.jobs.contains(name) {
                bail!("Work returned an unknown job identifier");
            }
            let Some(status) = terminal(&row["status"]) else {
                return Ok(None);
            };
            // GitLab durations are fractional seconds; the approved receipt is
            // integer seconds. Round up locally, never deserialize work's float
            // into the wire type or retain its textual representation.
            let duration = match row["duration"].as_f64() {
                Some(n) if n.is_finite() && (0.0..=86400.0).contains(&n) => n.ceil() as u32,
                None if row["duration"].is_null()
                    && matches!(status, Status::Skipped | Status::Canceled) =>
                {
                    0
                }
                _ => bail!("Invalid CI job duration"),
            };
            jobs.push(Job {
                name: name.into(),
                status,
                duration_seconds: duration,
            });
        }
        if rows.len() < 100 {
            complete = true;
            break;
        }
    }
    if !complete {
        bail!("CI jobs pagination exceeds bounds");
    }
    let report = api(&p.work_api, "GET", &format!("{prefix}/test_report"), None)?;
    let suites = report["test_suites"]
        .as_array()
        .context("Missing CI test report inventory")?;
    let mut failing_tests = Vec::new();
    for suite in suites {
        for test in suite["test_cases"]
            .as_array()
            .context("Invalid CI test suite")?
        {
            match test["status"].as_str() {
                Some("failed" | "error") => {
                    let name = test["name"]
                        .as_str()
                        .context("Missing CI test identifier")?;
                    // Return only a home-derived identifier. JUnit producers
                    // qualify Rust names with modules; no work-authored prefix
                    // may cross the return boundary.
                    let canonical = name.rsplit("::").next().unwrap_or(name);
                    if !inventory.tests.contains(canonical) {
                        bail!("Work returned an unknown test identifier");
                    }
                    failing_tests.push(canonical.to_owned());
                }
                Some("success" | "skipped") => {}
                _ => bail!("Unknown CI test state"),
            }
        }
    }
    failing_tests.sort();
    failing_tests.dedup();
    let mut receipt = Receipt {
        version: 1,
        kind: "ci-result".into(),
        sha: j.sha.clone(),
        attempt: j.attempt,
        status,
        jobs,
        failing_tests,
        digest: String::new(),
    };
    let bytes = receipt.seal()?;
    Receipt::parse(&bytes, inventory, &j.sha, j.attempt)?;
    Ok(Some(bytes))
}
fn pending_bytes(intake: &Intake, j: &Journal) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for part in 0..j.receipt_parts {
        bytes.extend(
            intake
                .private
                .read(&format!("ci-pending-{}-{part}", j.attempt))?
                .context("Missing pending CI receipt")?,
        );
    }
    Ok(bytes)
}
fn poll(intake: &Intake, p: &Profile, json_format: bool) -> Result<()> {
    let root = source(intake)?;
    let home = refs(&root, &p.home_remote)?;
    let work = refs(&root, &p.work_remote)?;
    // One attempt per invocation bounds total work; all attempt/deadline state
    // survives timer restarts. Later requests cannot replenish an old budget.
    let cursor = intake
        .private
        .read("ci-poll-cursor")?
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    // Persist selection before network I/O, so even a failed/slow attempt
    // cannot starve subsequent requests on the next timer tick.
    for (n, sha) in home
        .range((
            std::ops::Bound::Excluded(cursor),
            std::ops::Bound::Unbounded,
        ))
        .chain(home.range(..=cursor))
    {
        if work.get(n) != Some(sha) {
            bail!("Home/work retained request refs disagree");
        }
        let mut j = journal(intake, p, *n, sha)?;
        if j.done {
            continue;
        }
        intake
            .private
            .write("ci-poll-cursor", n.to_string().as_bytes())?;
        let path = format!("ci-results/{sha}/{n}.json");
        if j.receipt_parts > 0 {
            let bytes = pending_bytes(intake, &j)?;
            intake.ci_validate(&path, &bytes)?;
            intake.append(&path, &bytes)?;
            j.done = true;
            save(intake, &j)?;
            output(json_format, format!("CI attempt {n}: receipt published"));
            return Ok(());
        }
        if chrono::Utc::now().timestamp() >= j.deadline || j.polls >= p.max_polls {
            // An expired run is a terminal cancellation, never a forever
            // pending native check. Only home-derived job names are returned.
            fetch(&root, &p.home_remote, sha)?;
            let inventory = inventory(&root, sha)?;
            let mut receipt = Receipt {
                version: 1,
                kind: "ci-result".into(),
                sha: sha.clone(),
                attempt: *n,
                status: Status::Canceled,
                jobs: inventory
                    .jobs
                    .iter()
                    .map(|name| Job {
                        name: name.clone(),
                        status: Status::Canceled,
                        duration_seconds: 0,
                    })
                    .collect(),
                failing_tests: vec![],
                digest: String::new(),
            };
            let bytes = receipt.seal()?;
            for (part, chunk) in bytes.chunks(32768).enumerate() {
                intake
                    .private
                    .write(&format!("ci-pending-{n}-{part}"), chunk)?;
            }
            j.receipt_parts = bytes.chunks(32768).len();
            save(intake, &j)?;
            intake.append(&path, &bytes)?;
            j.done = true;
            save(intake, &j)?;
            output(
                json_format,
                format!("CI attempt {n}: expired; cancellation receipt published"),
            );
            return Ok(());
        }
        j.polls += 1;
        save(intake, &j)?;
        fetch(&root, &p.home_remote, sha)?;
        let inventory = inventory(&root, sha)?;
        let branch = format!("refs/heads/ci/{n}");
        immutable_push(&root, &p.work_remote, sha, &branch)?;
        if j.pipeline.is_none() {
            let v = api(
                &p.work_api,
                "GET",
                &format!(
                    "/projects/{}/pipelines?sha={sha}&ref=ci%2F{n}&per_page=100",
                    p.work_api.project
                ),
                None,
            )?;
            let rows = v.as_array().context("Invalid CI pipeline list")?;
            if rows.len() >= 100 {
                bail!("CI pipeline association exceeds bound");
            }
            let matching: Vec<_> = rows
                .iter()
                .filter(|r| {
                    r["sha"].as_str() == Some(sha.as_str())
                        && r["ref"].as_str() == Some(format!("ci/{n}").as_str())
                })
                .collect();
            if matching.len() > 1 {
                bail!("Ambiguous CI pipeline association; manual recovery required");
            }
            j.pipeline = matching
                .first()
                .map(|r| {
                    r["id"]
                        .as_u64()
                        .filter(|n| *n > 0)
                        .context("Invalid pipeline ID")
                })
                .transpose()?;
            save(intake, &j)?;
        }
        if j.pipeline.is_none() {
            output(
                json_format,
                format!("CI attempt {n}: awaiting pipeline registration"),
            );
            return Ok(());
        }
        match pipeline_receipt(p, &j, &inventory)? {
            None => output(json_format, format!("CI attempt {n}: pipeline pending")),
            Some(bytes) => {
                // Private files have a 64-KiB cap; journal is committed only
                // after all bounded chunks are durable. Partial writes retry.
                for (part, chunk) in bytes.chunks(32768).enumerate() {
                    intake
                        .private
                        .write(&format!("ci-pending-{n}-{part}"), chunk)?;
                }
                j.receipt_parts = bytes.chunks(32768).len();
                save(intake, &j)?;
                intake.append(&path, &bytes)?;
                j.done = true;
                save(intake, &j)?;
                output(json_format, format!("CI attempt {n}: receipt published"));
            }
        }
        return Ok(());
    }
    output(
        json_format,
        "CI relay: no pending admitted requests".to_string(),
    );
    Ok(())
}
fn publish_status(e: &Endpoint, r: &Receipt) -> Result<()> {
    let state = if r.status == Status::Success {
        "success"
    } else {
        "failure"
    };
    let (path, body) = if e.forge == "github" {
        (
            format!("/repos/{}/statuses/{}", e.project, r.sha),
            json!({"state": state, "context": CONTEXT, "description": format!("attempt {} {}", r.attempt, r.digest)}),
        )
    } else {
        (
            format!("/projects/{}/statuses/{}", e.project, r.sha),
            json!({"state": if state == "failure" {"failed"} else {state}, "name": CONTEXT, "description": format!("attempt {} {}", r.attempt, r.digest)}),
        )
    };
    api(e, "POST", &path, Some(body))?;
    Ok(())
}
fn request_pending(intake: &Intake, e: &Endpoint, attempt: u32, sha: &str) -> Result<()> {
    let path = if e.forge == "github" {
        format!("/repos/{}/commits/{sha}/statuses?per_page=100", e.project)
    } else {
        format!(
            "/projects/{}/repository/commits/{sha}/statuses?all=true&per_page=100",
            e.project
        )
    };
    let value = api(e, "GET", &path, None)?;
    let rows = value.as_array().context("Invalid native status response")?;
    if rows.len() >= 100 {
        bail!("Native status reconciliation exceeds bounded page");
    }
    let description = format!("attempt {attempt} pending");
    let prefix = format!("attempt {attempt} ");
    let pending = format!("ci-request-pending-{attempt}.json");
    if rows.iter().any(|row| {
        row[if e.forge == "github" {
            "context"
        } else {
            "name"
        }]
        .as_str()
            == Some(CONTEXT)
            && row["description"]
                .as_str()
                .is_some_and(|d| d.starts_with(&prefix))
    }) {
        intake.private.remove(&pending)?;
        return Ok(());
    }
    if intake.private.read(&pending)?.is_some() {
        bail!("Uncertain pending CI status; reconcile native status before retrying request");
    }
    intake.private.write(&pending, sha.as_bytes())?;
    let (path, body) = if e.forge == "github" {
        (
            format!("/repos/{}/statuses/{sha}", e.project),
            json!({"state": "pending", "context": CONTEXT, "description": description}),
        )
    } else {
        (
            format!("/projects/{}/statuses/{sha}", e.project),
            json!({"state": "pending", "name": CONTEXT, "description": description}),
        )
    };
    api(e, "POST", &path, Some(body))?;
    intake.private.remove(&pending)?;
    Ok(())
}
fn status_observed(e: &Endpoint, r: &Receipt) -> Result<bool> {
    let path = if e.forge == "github" {
        format!(
            "/repos/{}/commits/{}/statuses?per_page=100",
            e.project, r.sha
        )
    } else {
        format!(
            "/projects/{}/repository/commits/{}/statuses?all=true&per_page=100",
            e.project, r.sha
        )
    };
    let value = api(e, "GET", &path, None)?;
    let rows = value.as_array().context("Invalid native status response")?;
    if rows.len() >= 100 {
        bail!("Native status reconciliation exceeds bounded page");
    }
    let description = format!("attempt {} {}", r.attempt, r.digest);
    let expected = if r.status == Status::Success {
        "success"
    } else if e.forge == "github" {
        "failure"
    } else {
        "failed"
    };
    let matching: Vec<_> = rows
        .iter()
        .filter(|row| {
            row[if e.forge == "github" {
                "context"
            } else {
                "name"
            }]
            .as_str()
                == Some(CONTEXT)
                && row["description"].as_str() == Some(description.as_str())
        })
        .collect();
    if matching.len() > 1
        || matching.iter().any(|row| {
            row[if e.forge == "github" {
                "state"
            } else {
                "status"
            }]
            .as_str()
                != Some(expected)
        })
    {
        bail!("Conflicting native CI status evidence");
    }
    Ok(!matching.is_empty())
}
// Display-only evidence; this never grants receipt or publication authority.
// trace:TASK-3452 | ai:codex
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SkippedAttempt {
    pub sha: String,
    pub attempt: u32,
    pub reason: String,
    pub command: String,
}
impl SkippedAttempt {
    pub(crate) fn message(&self) -> String {
        format!(
            "CI SHA {} attempt {}: {}; run `{}`",
            self.sha, self.attempt, self.reason, self.command
        )
    }
}
const SKIPPED: &str = "ci-skipped-results.json";

// Refresh alongside the validated local inventory, including exact reads.
pub(super) fn record_skipped(
    intake: &Intake,
    files: &BTreeMap<String, Vec<u8>>,
    requests: &BTreeMap<u32, String>,
) -> Result<Vec<SkippedAttempt>> {
    let mut latest = BTreeMap::new();
    for (n, sha) in requests {
        latest.insert(sha, n);
    }
    let mut skipped = Vec::new();
    for (path, bytes) in files
        .iter()
        .filter(|(path, _)| path.starts_with("ci-results/"))
    {
        let r: Receipt = serde_json::from_slice(bytes)?;
        // A validator upgrade can drop verification proof without undoing a
        // completed native publication. Such rows need no operator action.
        // trace:TASK-3452 | ai:codex
        if !intake.ci_verified(path) && latest.get(&r.sha) == Some(&&r.attempt) {
            let published = intake
                .private
                .read(&format!("ci-status-{}.json", r.attempt))?;
            if published.as_deref() == Some(r.digest.as_bytes()) {
                continue;
            }
            skipped.push(SkippedAttempt {
                command: format!(
                    "aida intent ci read --attempt {} --sha {}",
                    r.attempt, r.sha
                ),
                sha: r.sha,
                attempt: r.attempt,
                reason: "outside the 32-attempt revalidation window".into(),
            });
        }
    }
    // Keep each display-cache file below Private's 64-KiB bound even
    // when a long retained history has many distinct pending SHAs.
    let old_parts: usize = intake
        .private
        .read(SKIPPED)?
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(0);
    let parts = skipped.len().div_ceil(100);
    for (index, rows) in skipped.chunks(100).enumerate() {
        intake.private.write(
            &format!("ci-skipped-results-{index}.json"),
            &serde_json::to_vec(rows)?,
        )?;
    }
    intake
        .private
        .write(SKIPPED, &serde_json::to_vec(&parts)?)?;
    for index in parts..old_parts {
        intake
            .private
            .remove(&format!("ci-skipped-results-{index}.json"))?;
    }
    Ok(skipped)
}

pub(crate) fn awaiting_skipped(root: &Path) -> Vec<SkippedAttempt> {
    let read = || -> Result<Vec<SkippedAttempt>> {
        let path = aida_core::allocation_authority::common_dir(root)?
            .join("aida-intake")
            .join(SKIPPED);
        let bytes = crate::observation_envelope::read_bounded(&path, 65536)?;
        let parts: usize = serde_json::from_slice(&bytes)?;
        let mut rows = Vec::new();
        for index in 0..parts {
            let bytes = crate::observation_envelope::read_bounded(
                &path.with_file_name(format!("ci-skipped-results-{index}.json")),
                65536,
            )?;
            rows.extend(serde_json::from_slice::<Vec<SkippedAttempt>>(&bytes)?);
        }
        Ok(rows)
    };
    read().unwrap_or_default()
}

fn process(intake: &Intake, p: &Profile, json_format: bool) -> Result<()> {
    intake.sync()?;
    let files = intake.files()?;
    let snapshot = Snapshot::load(intake)?;
    let mut latest = BTreeMap::<String, u32>::new();
    for (n, sha) in &snapshot.requests {
        latest
            .entry(sha.clone())
            .and_modify(|old| *old = (*old).max(*n))
            .or_insert(*n);
    }
    let skipped = record_skipped(intake, &files, &snapshot.requests)?;
    // Report the skipped inventory even when a later publication step fails.
    // trace:TASK-3452 | ai:codex
    let result = (|| -> Result<String> {
        for (path, bytes) in files
            .iter()
            .filter(|(path, _)| path.starts_with("ci-results/"))
        {
            // Only inventory-verified history grants receipt authority. Select
            // current attempts before source work; audit-only history cannot publish.
            let r: Receipt = serde_json::from_slice(bytes)?;
            if !intake.ci_verified(path) || latest.get(&r.sha) != Some(&r.attempt) {
                continue;
            }
            let marker = format!("ci-status-{}.json", r.attempt);
            if intake.private.read(&marker)?.as_deref() == Some(r.digest.as_bytes()) {
                continue;
            }
            let r = snapshot.validate(path, bytes)?;
            let marker = format!("ci-status-{}.json", r.attempt);
            if let Some(old) = intake.private.read(&marker)? {
                if old != r.digest.as_bytes() {
                    bail!("CI status processing identity conflict");
                }
                continue;
            }
            // Persist uncertainty before the POST. A crash/timeout never blindly
            // replays a status side effect. Operator reconciles native status and
            // removes this pending marker only after establishing no write happened.
            let pending = format!("ci-status-pending-{}.json", r.attempt);
            let endpoint = p
                .home_api
                .as_ref()
                .context("Home-only status endpoint required")?;
            if status_observed(endpoint, &r)? {
                intake.private.write(&marker, r.digest.as_bytes())?;
                intake.private.remove(&pending)?;
                return Ok(format!(
                    "CI attempt {}: native status reconciled",
                    r.attempt
                ));
            }
            if intake.private.read(&pending)?.is_some() {
                bail!("Uncertain native CI status publication; reconcile pending marker with forge before retrying");
            }
            intake.private.write(&pending, r.digest.as_bytes())?;
            // Re-pin admission immediately before the write. A new attempt for this
            // SHA supersedes older receipts even if the new receipt has not arrived.
            let current = refs(&source(intake)?, &p.home_remote)?;
            if current
                .iter()
                .any(|(n, sha)| sha == &r.sha && *n > r.attempt)
            {
                intake.private.remove(&pending)?;
                continue;
            }
            if current.get(&r.attempt) != Some(&r.sha) {
                bail!("Retained CI request changed before status publication");
            }
            publish_status(
                p.home_api
                    .as_ref()
                    .context("Home-only status endpoint required")?,
                &r,
            )?;
            intake.private.write(&marker, r.digest.as_bytes())?;
            intake.private.remove(&pending)?;
            return Ok(format!("CI attempt {}: native status published", r.attempt));
        }
        Ok("CI intake: no unprocessed current results".to_string())
    })();
    if json_format {
        match &result {
            Ok(message) => println!("{}", json!({"message": message, "skipped": skipped})),
            Err(error) => println!(
                "{}",
                json!({"error": error.to_string(), "skipped": skipped})
            ),
        }
    } else {
        for item in &skipped {
            println!("{}", item.message());
        }
        if let Ok(message) = &result {
            println!("{message}");
        }
    }
    result.map(|_| ())
}
fn probe(intake: &Intake, root: &Path, sha: &str) -> Result<()> {
    // The actual separately enrolled return URL/credential is used. Passing a
    // flag alone cannot assert that the server restricts this credential.
    for target in [
        format!("refs/heads/aida-ci-denied-{}", uuid::Uuid::new_v4()),
        format!("refs/tags/aida-ci-denied-{}", uuid::Uuid::new_v4()),
    ] {
        if intent_intake::git(
            root,
            &[
                "push",
                "--no-verify",
                "--no-follow-tags",
                "--",
                &intake.profile_remote(),
                &format!("{sha}:{target}"),
            ],
            &[],
        )
        .is_ok()
        {
            bail!("Hub accepted a forbidden credential probe; CI enrollment refused. Operator must remove the unexpected probe ref and repair server enforcement");
        }
        // A transport outage must not be mistaken for a credential restriction.
        git(
            root,
            &["ls-remote", "--refs", "--", &intake.profile_remote()],
        )?;
    }
    Ok(())
}
fn output(json_format: bool, message: String) {
    if json_format {
        println!("{}", json!({"message": message}));
    } else {
        println!("{message}");
    }
}
pub(crate) fn run(intake: &Intake, command: &CiCommand, json_format: bool) -> Result<()> {
    if let CiCommand::Enroll { file, reviewed } = command {
        if !reviewed {
            bail!("CI enrollment requires reviewed configuration");
        }
        let bytes = crate::observation_envelope::read_bounded(file, 16384)?;
        let p: Profile = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid CI profile schema"))?;
        validate_profile(&p)?;
        let canonical = serde_json::to_vec(&p)?;
        if let Some(old) = intake.private.read(CONFIG)? {
            if old != canonical {
                bail!("CI already enrolled; use a separate clone for policy changes");
            }
        }
        let root = source(intake)?;
        let requests = refs(&root, &p.home_remote)?;
        let sha = requests.values().next().context(
            "CI enrollment requires at least one retained home request for live restriction probes",
        )?;
        fetch(&root, &p.home_remote, sha)?;
        probe(intake, &root, sha)?;
        intake.private.write(CONFIG, &canonical)?;
        output(
            json_format,
            "Enrolled CI profile; return credential branch/tag probes refused".to_string(),
        );
        return Ok(());
    }
    let p = profile(intake)?;
    match command {
        CiCommand::Request { attempt, sha } => {
            if p.role != "home" || *attempt == 0 || !ci_receipt::oid(sha) {
                bail!("Explicit home-assigned positive attempt and full commit SHA required");
            }
            let root = source(intake)?;
            fetch(&root, &p.home_remote, sha)?;
            inventory(&root, sha)?;
            let requests = refs(&root, &p.home_remote)?;
            if requests
                .iter()
                .any(|(n, target)| *n > *attempt && target == sha)
            {
                bail!("A newer CI attempt already supersedes this request");
            }
            let target = format!("refs/aida/ci/{attempt}");
            immutable_push(&root, &p.home_remote, sha, &target)?;
            request_pending(
                intake,
                p.home_api
                    .as_ref()
                    .context("Home status endpoint required")?,
                *attempt,
                sha,
            )?;
            immutable_push(&root, &p.work_remote, sha, &target)?;
            output(
                json_format,
                format!("CI attempt {attempt}: retained exact-SHA request published"),
            );
        }
        CiCommand::Poll => {
            if p.role != "work" {
                bail!("CI poll requires a work profile without home API credentials");
            }
            poll(intake, &p, json_format)?;
        }
        CiCommand::Process => {
            if p.role != "home" {
                bail!("CI processing requires a home profile");
            }
            process(intake, &p, json_format)?;
        }
        CiCommand::Read { attempt, sha } => {
            if sha.as_ref().is_some_and(|s| !ci_receipt::oid(s)) || *attempt == Some(0) {
                bail!("Exact positive attempt and full lowercase SHA required");
            }
            intake.sync()?;
            let files = intake.files()?;
            let requested = attempt
                .zip(sha.as_deref())
                .map(|(n, sha)| format!("ci-results/{sha}/{n}.json"));
            if let Some(path) = &requested {
                let bytes = files
                    .get(path)
                    .context("No result for requested attempt/SHA")?;
                intake.verify_ci_read(path, bytes)?;
                let snapshot = Snapshot::load(intake)?;
                record_skipped(intake, &files, &snapshot.requests)?;
            }
            let rows: Vec<_> = files.iter().filter(|(path, _)| path.starts_with("ci-results/")
                && requested.as_ref().is_none_or(|p| p == *path))
                .map(|(path, bytes)| -> Result<Value> {
                    Ok(json!({"path": path, "verification": if intake.ci_verified(path) { "verified" } else { "unverified-history" },
                        "audit": serde_json::from_slice::<Receipt>(bytes)?}))
                }).collect::<Result<_>>()?;
            if json_format {
                println!("{}", serde_json::to_string(&rows)?);
            } else {
                for row in rows {
                    println!(
                        "{} {} {}",
                        row["path"].as_str().unwrap(),
                        row["verification"].as_str().unwrap(),
                        row["audit"]
                    );
                }
            }
        }
        CiCommand::Enroll { .. } => unreachable!(),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // trace:TASK-3452 | ai:codex
    #[test]
    fn task3452_history_window_reports_skipped_attempts() {
        let executable = std::env::current_exe().unwrap();
        let binary = executable.parent().unwrap().parent().unwrap().join("aida");
        assert!(
            binary.is_file(),
            "build the CLI in this profile before running this regression"
        );
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/test_work_ci_relay.py");
        let mut command = std::process::Command::new("python3");
        command
            .arg(script)
            .args([
                "WorkCiTests.test_33_attempts_report_unverified_latest_result",
                "WorkCiTests.test_cold_history_bounds_inventory_and_exact_old_read",
                "WorkCiTests.test_warm_history_bounds_new_inventory_and_exact_old_read",
            ])
            .env("AIDA_TEST_BIN", binary);
        let (status, stdout, stderr) = aida_core::transport_policy::bounded_with_stderr(
            command,
            &[],
            1024 * 1024,
            std::time::Instant::now() + Duration::from_secs(480),
        )
        .unwrap();
        assert!(
            status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr)
        );
    }
    // Exercise the actual on-disk awaiting reader, including histories larger
    // than the former arbitrary 82-part display ceiling.
    // trace:TASK-3452 | ai:codex
    #[test]
    fn awaiting_reads_large_skipped_cache() {
        let (temp, _) = repository("name: test", ".github/workflows/test.yml");
        let dir = aida_core::allocation_authority::common_dir(temp.path())
            .unwrap()
            .join("aida-intake");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(SKIPPED), b"83").unwrap();
        for index in 0..83 {
            let row = SkippedAttempt {
                sha: "a".repeat(40),
                attempt: index + 1,
                reason: "outside the 32-attempt revalidation window".into(),
                command: format!(
                    "aida intent ci read --attempt {} --sha {}",
                    index + 1,
                    "a".repeat(40)
                ),
            };
            fs::write(
                dir.join(format!("ci-skipped-results-{index}.json")),
                serde_json::to_vec(&vec![row]).unwrap(),
            )
            .unwrap();
        }
        let rows = awaiting_skipped(temp.path());
        assert_eq!(rows.len(), 83);
        assert_eq!(rows[82].attempt, 83);
        assert!(rows[82].message().contains(&rows[82].command));
    }

    fn repository(yaml: &str, path: &str) -> (tempfile::TempDir, String) {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "-q", "--template=", "-b", "main"]).unwrap();
        let file = temp.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, yaml).unwrap();
        fs::write(
            temp.path().join("lib.rs"),
            "mod nested { #[test] fn known() {} }",
        )
        .unwrap();
        git(temp.path(), &["add", "."]).unwrap();
        git(
            temp.path(),
            &[
                "-c",
                "maintenance.auto=false",
                "-c",
                "gc.auto=0",
                "commit",
                "-qm",
                "source",
            ],
        )
        .unwrap();
        let sha = git(temp.path(), &["rev-parse", "HEAD"]).unwrap();
        (temp, sha)
    }
    // trace:STORY-b6b1278890774ef993d2ebc327862470-408.ac0a391b | ai:codex
    #[test]
    fn pinned_inventory_rejects_indirect_jobs() {
        let (repo, sha) = repository("build:\n  script: cargo test\n", ".gitlab-ci.yml");
        let inv = inventory(repo.path(), &sha).unwrap();
        assert_eq!(inv.jobs, ["build".into()].into());
        assert_eq!(inv.tests, ["known".into()].into());
        for yaml in [
            "include: remote.yml\nbuild:\n  script: true\n",
            "build:\n  script: true\n  parallel: 2\n",
            "build:\n  script: true\n  rules: []\n",
            "build:\n  extends: .hidden\n",
            "build:\n  script: true\nbuild:\n  script: false\n",
        ] {
            let (repo, sha) = repository(yaml, ".gitlab-ci.yml");
            assert!(inventory(repo.path(), &sha).is_err(), "{yaml}");
        }
    }
    #[test]
    fn github_inventory_rejects_matrices_and_dynamic_names() {
        for (yaml, valid) in [
            (
                "jobs:\n  build:\n    runs-on: ubuntu-latest\n    steps: []\n",
                true,
            ),
            (
                "jobs:\n  build:\n    strategy:\n      matrix: {}\n    steps: []\n",
                false,
            ),
            (
                "jobs:\n  build:\n    name: '${{ matrix.name }}'\n    steps: []\n",
                false,
            ),
            ("jobs:\n  build:\n    uses: other.yml\n", false),
        ] {
            let (repo, sha) = repository(yaml, ".github/workflows/check.yml");
            assert_eq!(inventory(repo.path(), &sha).is_ok(), valid);
        }
    }
}
