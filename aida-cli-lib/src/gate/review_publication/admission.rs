//! Single-hub admission. Temporary Git object databases never change the code
//! checkout, store index or shared FETCH_HEAD. Only an exact accepted CAS admits.
// trace:STORY-2492 | ai:codex
use super::*;

const REF: &str = "refs/aida/review-admission";
const TABLE: &str = "admission.json";
const ATTEMPTS: usize = 3;

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut cmd = aida_core::git_ops::host_git_command(root);
    if !aida_core::git_ops::is_store_tree(root) {
        // The invocation-private bare object DB never runs caller templates.
        cmd.args(["-c", "core.hooksPath=/dev/null"]);
    }
    cmd.args(args).stdin(Stdio::null());
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_NAMESPACE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        cmd.env_remove(key);
    }
    run(cmd)
}

pub(crate) fn require_pinned(root: &Path, expected: &Option<Hub>) -> Result<()> {
    ensure!(
        Hub::load(root)? == *expected,
        "admission configuration changed since reservation; no dispatch"
    );
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Hub {
    repository: String,
    url: String,
    reference: String,
}
impl Hub {
    pub(crate) fn load(root: &Path) -> Result<Option<Self>> {
        let config = crate::trusted_config::read_trusted_config_toml_local(root);
        let table: toml::Value = toml::from_str(config.as_deref().unwrap_or(""))?;
        let review = table.get("review");
        let url = review.and_then(|r| r.get("admission_hub"));
        let reference = review.and_then(|r| r.get("admission_ref"));
        if url.is_none() && reference.is_none() && publisher(root)?.is_none() {
            return Ok(None);
        }
        Self::new(
            url.and_then(|v| v.as_str())
                .context("explicit trusted [review] admission_hub required")?,
            reference
                .and_then(|v| v.as_str())
                .context("explicit trusted [review] admission_ref required")?,
        )
        .map(Some)
    }
    pub(crate) fn validate_repository(&self, repository: &str) -> Result<()> {
        ensure!(self.repository == repository,
            "admission_mirror_refused: watcher repository must match the configured GitHub authority; mirrors never admit");
        Ok(())
    }
    fn new(url: &str, reference: &str) -> Result<Self> {
        ensure!(
            matches!(crate::forge::detect_forge_kind(url), crate::forge::ForgeKind::GitHub)
                && crate::forge::forge_host_of(url).as_deref() == Some("github.com")
                && crate::forge::project_path_of(url).is_some()
                && (url.starts_with("https://github.com/") || url.starts_with("git@github.com:") || url.starts_with("ssh://git@github.com/")),
            "admission_mirror_refused: admission_hub must name the explicit GitHub authority; mirrors never admit"
        );
        ensure!(
            reference == REF,
            "unsupported admission_ref; expected {REF}"
        );
        let project = crate::forge::project_path_of(url).context("missing admission repository")?;
        ensure!(
            project.split('/').count() == 2
                && project.split('/').all(|p| !p.is_empty()
                    && p != "."
                    && p != ".."
                    && p.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))),
            "invalid admission repository"
        );
        Ok(Self {
            repository: format!("github:github.com/{project}"),
            url: url.into(),
            reference: reference.into(),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    Reserved,
    Running,
    Terminal,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transition {
    version: u32,
    kind: String,
    repository: String,
    hub: String,
    reference: String,
    change: String,
    head: String,
    id: Uuid,
    predecessor: Option<Uuid>,
    token: Uuid,
    node: String,
    session: String,
    state: State,
    evidence: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    version: u32,
    // One causal chain per change; old active heads fence new heads too.
    changes: BTreeMap<String, Vec<Signed>>,
}
fn active(state: &State) -> bool {
    matches!(state, State::Reserved | State::Running)
}

fn chain(hub: &Hub, key: &str, records: &[Signed], roster: &Roster) -> Result<Vec<Transition>> {
    let mut out: Vec<Transition> = Vec::new();
    ensure!(records.len() <= 2048, "admission history budget exhausted");
    let started = std::time::Instant::now();
    for signed in records {
        ensure!(
            started.elapsed() < Duration::from_secs(20),
            "admission verification deadline exhausted"
        );
        let t: Transition = serde_json::from_str(&signed.payload)?;
        ensure!(
            serde_json::to_string(&t)? == signed.payload,
            "noncanonical admission transition"
        );
        ensure!(
            t.version == 1
                && t.kind == "review-admission"
                && t.repository == hub.repository
                && t.hub == hub.url
                && t.reference == hub.reference
                && t.change == key,
            "admission transition scope/version mismatch"
        );
        ensure!(
            t.head.len() == 40
                && t.head
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                && !t.session.is_empty()
                && t.session.len() <= 256,
            "invalid admission head/session"
        );
        ensure!(
            t.token.get_version_num() == 4 && t.id.get_version_num() == 4,
            "admission requires unpredictable v4 claim/transition IDs"
        );
        verify_signature(&t.node, signed, roster)?;
        let prior = out.last();
        ensure!(
            t.predecessor == prior.map(|p| p.id) && !out.iter().any(|p| p.id == t.id),
            "invalid admission predecessor"
        );
        match &t.state {
            State::Reserved => {
                ensure!(
                    prior.is_none_or(|p| !active(&p.state)),
                    "active admission cannot be replaced"
                );
                ensure!(
                    !out.iter()
                        .any(|p| p.token == t.token
                            || (p.head == t.head && p.state == State::Terminal)),
                    "consumed token/head cannot be readmitted"
                );
                ensure!(
                    t.evidence.is_none(),
                    "reservation cannot claim exit evidence"
                );
            }
            state => {
                let p = prior.context("admission transition without reservation")?;
                ensure!(
                    p.head == t.head
                        && p.node == t.node
                        && p.session == t.session
                        && p.token == t.token,
                    "admission owner/token changed"
                );
                ensure!(
                    matches!(
                        (&p.state, state),
                        (State::Reserved, State::Running)
                            | (State::Reserved, State::Cancelled)
                            | (State::Running, State::Terminal)
                    ),
                    "invalid admission state transition"
                );
                ensure!(
                    t.evidence
                        .as_ref()
                        .is_some_and(|e| !e.is_empty() && e.len() <= 4096),
                    "admission transition requires evidence"
                );
            }
        }
        out.push(t);
    }
    Ok(out)
}

fn remote_tip(repo: &Path, hub: &Hub) -> Result<Option<String>> {
    let output = text(
        git(repo, &["ls-remote", "--refs", &hub.url, &hub.reference])
            .with_context(|| format!("admission_hub_unreachable: {} {}", hub.url, hub.reference))?,
    )?;
    if output.is_empty() {
        return Ok(None);
    }
    let parts: Vec<_> = output.split_whitespace().collect();
    ensure!(
        parts.len() == 2
            && parts[1] == hub.reference
            && parts[0].len() == 40
            && parts[0].bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid admission hub tip"
    );
    Ok(Some(parts[0].into()))
}
fn read_table(repo: &Path, hub: &Hub) -> Result<(Option<String>, Table)> {
    let tip = remote_tip(repo, hub)?;
    let Some(tip) = tip else {
        return Ok((
            None,
            Table {
                version: 1,
                ..Table::default()
            },
        ));
    };
    git(
        repo,
        &[
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            &hub.url,
            &tip,
        ],
    )
    .with_context(|| format!("admission_hub_unreachable: {}", hub.url))?;
    let bytes = blob(repo, &tip, TABLE)?.context("missing admission state table")?;
    let table: Table = serde_json::from_slice(&bytes)?;
    ensure!(
        table.version == 1 && table.changes.len() <= 2000,
        "invalid/oversized admission table"
    );
    Ok((Some(tip), table))
}
fn private_repo() -> Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    git(dir.path(), &["init", "--bare", "-q"])?;
    Ok(dir)
}
fn commit_table(repo: &Path, tip: Option<&str>, table: &Table) -> Result<String> {
    let path = repo.join(TABLE);
    let bytes = serde_json::to_vec(table)?;
    ensure!(bytes.len() <= CAP, "admission table budget exhausted");
    std::fs::write(&path, bytes)?;
    let blob = text(git(
        repo,
        &[
            "hash-object",
            "-w",
            path.to_str().context("admission path")?,
        ],
    )?)?;
    // The entire table is one bounded blob. A private index preserves unrelated
    // entries from the accepted parent without touching either worktree index.
    if let Some(tip) = tip {
        git(repo, &["read-tree", tip])?;
    }
    git(
        repo,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            "100644",
            &blob,
            TABLE,
        ],
    )?;
    let tree = text(git(repo, &["write-tree"])?)?;
    let mut args = vec![
        "-c",
        "user.name=AIDA review admission",
        "-c",
        "user.email=aida@localhost",
        "commit-tree",
        &tree,
    ];
    if let Some(tip) = tip {
        args.extend(["-p", tip]);
    }
    args.extend(["-m", "review: signed admission transition"]);
    text(git(repo, &args)?)
}

/// Returns false for a winning peer or a previously consumed head. A push
/// failure is never success, even when a later read happens to find our token.
fn update(
    root: &Path,
    hub: &Hub,
    key: &str,
    head: &str,
    token: Uuid,
    state: State,
    evidence: Option<String>,
) -> Result<bool> {
    ensure!(
        key.strip_prefix("PR-").is_some_and(|id| id
            .parse::<u64>()
            .is_ok_and(|id| id > 0 && key == format!("PR-{id}"))),
        "admission_mirror_refused: only GitHub PRs admit; GitLab is a mirror"
    );
    let store = store(root).context("review admission needs a canonical store")?;
    aida_core::db::with_store_write_lock(&store, || {
        if state == State::Reserved
            && watcher_history(root, key)?.iter().any(|(body, _)| {
                rv::parse_recorded_verdict(body)
                    .is_some_and(|v| v.reviewed_sha.as_deref() == Some(head))
            })
        {
            return Ok(false);
        }
        let config =
            publisher(root)?.context("review admission requires an enrolled local publisher")?;
        let roster = roster(&store, &super::head(&store)?)?;
        let repo = private_repo()?;
        for _ in 0..ATTEMPTS {
            let (tip, mut table) = read_table(repo.path(), hub)?;
            let records = table.changes.entry(key.into()).or_default();
            let history = chain(hub, key, records, &roster)?;
            let prior = history.last();
            if state == State::Reserved {
                if prior.is_some_and(|p| active(&p.state))
                    || history
                        .iter()
                        .any(|p| p.head == head && p.state == State::Terminal)
                {
                    return Ok(false);
                }
            } else {
                let p = prior.context("missing accepted admission")?;
                ensure!(
                    p.token == token && p.node == config.node && p.head == head,
                    "admission token not owned by this node"
                );
                // Retrying a Running consume must never spawn again.
                ensure!(p.state != state, "admission token already consumed");
            }
            let t = Transition {
                version: 1,
                kind: "review-admission".into(),
                repository: hub.repository.clone(),
                hub: hub.url.clone(),
                reference: hub.reference.clone(),
                change: key.into(),
                head: head.into(),
                id: Uuid::new_v4(),
                predecessor: prior.map(|p| p.id),
                token,
                node: config.node.clone(),
                session: if state == State::Reserved {
                    std::env::var("AIDA_SESSION_ID")
                        .ok()
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| format!("watch:{token}"))
                } else {
                    prior.unwrap().session.clone()
                },
                state: state.clone(),
                evidence: evidence.clone(),
            };
            records.push(sign(&config, &t)?);
            chain(hub, key, records, &roster)?;
            let next = commit_table(repo.path(), tip.as_deref(), &table)?;
            let lease = format!(
                "--force-with-lease={}:{}",
                hub.reference,
                tip.as_deref().unwrap_or("")
            );
            let target = format!("{next}:{}", hub.reference);
            if let Err(error) = git(
                repo.path(),
                &["push", "--porcelain", &lease, &hub.url, &target],
            ) {
                let observed = remote_tip(repo.path(), hub)?;
                // Only a proven peer update allows reconstruction. Lost push
                // responses and connectivity errors retain the claim safely.
                if observed == tip || observed.as_deref() == Some(&next) {
                    return Err(error).context("admission_push_uncertain: no dispatch; reconcile accepted claims before retry");
                }
                continue;
            }
            let (_, accepted) = read_table(repo.path(), hub)?;
            let accepted = chain(
                hub,
                key,
                accepted
                    .changes
                    .get(key)
                    .context("accepted admission disappeared")?,
                &roster,
            )?;
            ensure!(
                accepted.iter().any(|r| r.id == t.id),
                "admission token was not confirmed; no dispatch"
            );
            return Ok(true);
        }
        bail!("admission CAS retry budget exhausted; no dispatch")
    })
}

pub(crate) fn reserve(root: &Path, key: &str, head: &str, token: &str) -> Result<bool> {
    let Some(hub) = Hub::load(root)? else {
        return Ok(true);
    };
    update(
        root,
        &hub,
        key,
        head,
        Uuid::parse_str(token)?,
        State::Reserved,
        None,
    )
}
pub(crate) fn transition(
    root: &Path,
    key: &str,
    head: &str,
    token: &str,
    state: State,
    evidence: String,
) -> Result<()> {
    let Some(hub) = Hub::load(root)? else {
        return Ok(());
    };
    ensure!(
        update(
            root,
            &hub,
            key,
            head,
            Uuid::parse_str(token)?,
            state,
            Some(evidence)
        )?,
        "admission transition refused"
    );
    Ok(())
}

/// Synchronize ordinary publications before forge discovery. The existing
/// checked store writer owns evidence reconciliation; no separate merge path.
pub(crate) fn sync(root: &Path) -> Result<()> {
    if Hub::load(root)?.is_none() {
        return Ok(());
    }
    let store = store(root).context("review admission needs canonical store")?;
    aida_core::db::with_store_write_lock(&store, || {
        aida_core::git_ops::ensure_store_write_safe(&store)?;
        ensure!(
            !aida_core::git_ops::has_changes_checked(&store)?,
            "review publication sync requires a clean store"
        );
        let branch = text(git(&store, &["symbolic-ref", "--short", "HEAD"])?)?;
        // Invocation-private ref; shared FETCH_HEAD and remote tracking refs
        // never become coordination state. Existing evidence gate owns FF.
        let reference = format!("refs/aida/review-sync/{}", Uuid::new_v4());
        let fetched = git(
            &store,
            &[
                "fetch",
                "--no-tags",
                "--no-write-fetch-head",
                "origin",
                &format!("refs/heads/{branch}:{reference}"),
            ],
        );
        let result =
            fetched.and_then(|_| aida_core::git_ops::fast_forward_store(&store, &reference));
        let cleanup = git(&store, &["update-ref", "-d", &reference]);
        result.context("review publication synchronization refused")?;
        cleanup?;
        Ok(())
    })
}

#[cfg(test)]
#[path = "tests/admission_tests.rs"]
mod tests;
