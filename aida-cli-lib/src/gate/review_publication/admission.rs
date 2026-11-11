//! Single-hub admission. Temporary Git object databases never change the code
//! checkout, store index or shared FETCH_HEAD. Only an exact accepted CAS admits.
// mandate-gate
// trace:STORY-2492 | ai:codex
// trace:TASK-b6b1278890774ef993d2ebc327862470-871 | ai:codex
use super::*;

const REF: &str = "refs/aida/review-admission";
const TABLE: &str = "admission.json";
const ATTEMPTS: usize = 3;

fn command(root: &Path) -> Command {
    let mut cmd = aida_core::git_ops::host_git_command(root);
    if !aida_core::git_ops::is_store_tree(root) {
        // The invocation-private bare object DB never runs caller templates.
        cmd.args(["-c", "core.hooksPath=/dev/null"]);
    }
    cmd.stdin(Stdio::null());
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
    cmd
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut cmd = command(root);
    cmd.args(args);
    run(cmd)
}

// Route credentials after clearing inherited Git configuration so routing's
// private config environment survives, including for explicit hub URLs.
// trace:STORY-2492 | ai:codex
fn remote_git(root: &Path, remote: &str, args: &[&str]) -> Result<Vec<u8>> {
    let mut cmd = command(root);
    cmd.args(args);
    aida_core::git_ops::configure_remote_credentials(&mut cmd, root, remote);
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
    Retry,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    anchor: Option<Signed>,
}
// A recovery reset is an explicit signed authority transition. Consumers verify
// its signature and scope, but do not compare shards with Git ancestors.
// Trusted writers must preserve chains and only reset through recovery; a buggy
// trusted writer can truncate a shard or replay a previously signed anchor.
// trace:TASK-3196 | ai:codex
fn validate_anchor(hub: &Hub, key: &str, table: &Table, roster: &Roster) -> Result<()> {
    if let Some(anchor) = &table.anchor {
        let value: serde_json::Value = serde_json::from_str(&anchor.payload)?;
        ensure!(
            serde_json::to_string(&value)? == anchor.payload
                && value["version"] == 1
                && value["kind"] == "review-admission-recovery"
                && value["change"] == key
                && value["repository"] == hub.repository
                && value["hub"] == hub.url
                && value["reference"] == hub.reference
                && value["all_reviewers_stopped"] == true
                && value["predecessor_tip"]
                    .as_str()
                    .is_some_and(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                && value["reason"]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty() && s.len() <= 4096),
            "invalid admission recovery anchor"
        );
        verify_signature(
            value["node"].as_str().context("recovery node missing")?,
            anchor,
            roster,
        )?;
    }
    Ok(())
}

fn active(state: &State) -> bool {
    matches!(state, State::Reserved | State::Retry | State::Running)
}

// Only a stopped, verdict-less attempt can authorize bounded same-head readmission.
// trace:BUG-2518 | ai:codex
pub(crate) const NO_VERDICT: &str = "NoVerdict: ";
fn retry_allowed(history: &[Transition], head: &str) -> bool {
    // A cancelled pre-spawn Retry did not consume a started attempt. Peel only
    // those exact pairs; unrelated heads or reservations cannot borrow authority.
    let mut authority = history;
    while let [rest @ .., retry, cancelled] = authority {
        if cancelled.state != State::Cancelled
            || retry.state != State::Retry
            || retry.head != head
            || cancelled.head != head
            || cancelled.token != retry.token
            || cancelled.predecessor != Some(retry.id)
        {
            break;
        }
        authority = rest;
    }
    authority.last().is_some_and(|p| {
        p.head == head
            && p.state == State::Terminal
            && p.evidence
                .as_deref()
                .is_some_and(|e| e.starts_with(NO_VERDICT))
    }) && history
        .iter()
        .filter(|p| p.head == head && p.state == State::Running)
        .count()
        < 3
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
            State::Reserved | State::Retry => {
                ensure!(
                    prior.is_none_or(|p| !active(&p.state)),
                    "active admission cannot be replaced"
                );
                ensure!(
                    !out.iter().any(|p| p.token == t.token)
                        && if t.state == State::Retry {
                            retry_allowed(&out, &t.head)
                        } else {
                            !out.iter()
                                .any(|p| p.head == t.head && p.state == State::Terminal)
                        },
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
                            | (State::Retry, State::Running)
                            | (State::Retry, State::Cancelled)
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

fn change_path(key: &str) -> Result<String> {
    ensure!(
        key.strip_prefix("PR-").is_some_and(|id| id
            .parse::<u64>()
            .is_ok_and(|n| n > 0 && key == format!("PR-{n}"))),
        "invalid admission change"
    );
    Ok(format!("changes/{key}.json"))
}

fn remote_tip(repo: &Path, hub: &Hub) -> Result<Option<String>> {
    let output = text(
        remote_git(
            repo,
            &hub.url,
            &["ls-remote", "--refs", &hub.url, &hub.reference],
        )
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
fn read_table(repo: &Path, hub: &Hub, key: &str) -> Result<(Option<String>, Table)> {
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
    remote_git(
        repo,
        &hub.url,
        &[
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            &hub.url,
            &tip,
        ],
    )
    .with_context(|| format!("admission_hub_unreachable: {}", hub.url))?;
    let bytes = match blob(repo, &tip, &change_path(key)?)? {
        Some(bytes) => bytes,
        None => match blob(repo, &tip, TABLE)? {
            Some(bytes) => bytes,
            None => {
                return Ok((
                    Some(tip),
                    Table {
                        version: 1,
                        ..Table::default()
                    },
                ))
            }
        },
    };
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
fn commit_table(repo: &Path, tip: Option<&str>, key: &str, table: &Table) -> Result<String> {
    let relative = change_path(key)?;
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
    // Each change is a bounded blob. A private index preserves unrelated
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
            &relative,
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

/// Returns false for a winning peer or an ineligible consumed head. A push
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
    update_with_observer(root, hub, key, head, token, state, evidence, || {})
}

// Both snapshots use a short store lock; hub I/O never holds that lock. Compare
// only admission evidence, so unrelated store writes cannot strand exit claims.
// trace:TASK-3196 | ai:codex
fn admission_evidence(
    root: &Path,
    hub: &Hub,
    key: &str,
    head: &str,
    state: &State,
) -> Result<Option<(Publisher, Roster)>> {
    let store = store(root).context("review admission needs a canonical store")?;
    aida_core::db::with_store_write_lock(&store, || {
        require_pinned(root, &Some(hub.clone()))?;
        if matches!(state, State::Reserved | State::Retry)
            && watcher_history(root, key)?.iter().any(|(body, _)| {
                rv::parse_recorded_verdict(body)
                    .is_some_and(|v| v.reviewed_sha.as_deref() == Some(head))
            })
        {
            return Ok(None);
        }
        let config =
            publisher(root)?.context("review admission requires an enrolled local publisher")?;
        let roster = roster(&store, &super::head(&store)?)?;
        Ok(Some((config, roster)))
    })
}

// The observer lets regressions move canonical evidence during the network
// window. Production uses the same path with a no-op observer.
// trace:TASK-3196 | ai:codex
#[allow(clippy::too_many_arguments)]
fn update_with_observer(
    root: &Path,
    hub: &Hub,
    key: &str,
    head: &str,
    token: Uuid,
    state: State,
    evidence: Option<String>,
    mut after_read: impl FnMut(),
) -> Result<bool> {
    let repo = private_repo()?;
    for _ in 0..ATTEMPTS {
        let Some((config, roster)) = admission_evidence(root, hub, key, head, &state)? else {
            return Ok(false);
        };
        let (tip, mut table) = read_table(repo.path(), hub, key)?;
        after_read();
        validate_anchor(hub, key, &table, &roster)?;
        table.changes.retain(|change, _| change == key);
        let records = table.changes.entry(key.into()).or_default();
        let history = chain(hub, key, records, &roster)?;
        let prior = history.last();
        let state = if state == State::Reserved {
            if prior.is_some_and(|p| active(&p.state)) {
                return Ok(false);
            }
            if history
                .iter()
                .any(|p| p.head == head && p.state == State::Terminal)
            {
                if !retry_allowed(&history, head) {
                    return Ok(false);
                }
                State::Retry
            } else {
                State::Reserved
            }
        } else {
            state.clone()
        };
        if !matches!(state, State::Reserved | State::Retry) {
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
            session: if matches!(state, State::Reserved | State::Retry) {
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
        let next = commit_table(repo.path(), tip.as_deref(), key, &table)?;
        let lease = format!(
            "--force-with-lease={}:{}",
            hub.reference,
            tip.as_deref().unwrap_or("")
        );
        let target = format!("{next}:{}", hub.reference);
        let Some((current_config, current_roster)) =
            admission_evidence(root, hub, key, head, &state)?
        else {
            return Ok(false);
        };
        if current_config.node != config.node
            || current_config.private_key != config.private_key
            || current_roster.nodes != roster.nodes
        {
            continue;
        }
        if let Err(error) = remote_git(
            repo.path(),
            &hub.url,
            &["push", "--porcelain", &lease, &hub.url, &target],
        ) {
            let observed = remote_tip(repo.path(), hub)?;
            // Only a proven peer update allows reconstruction. Lost push
            // responses and connectivity errors retain the claim safely.
            if observed == tip || observed.as_deref() == Some(&next) {
                return Err(error).context(
                    "admission_push_uncertain: no dispatch; reconcile accepted claims before retry",
                );
            }
            continue;
        }
        let (_, accepted) = read_table(repo.path(), hub, key)?;
        validate_anchor(hub, key, &accepted, &roster)?;
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
    bail!("admission CAS/evidence retry budget exhausted; no dispatch")
}

/// Explicit stopped-admission recovery. The direct-human gate is enforced by
/// `nodes`; no watcher, timeout, reboot or remote PID can invoke this implicitly.
/// Old signatures may be revoked or corrupt: the inspected exact Git tip, not
/// their validity, is the operator's recovery anchor. History remains in Git.
// trace:STORY-2492 | ai:codex
pub(super) fn reconcile(
    root: &Path,
    key: &str,
    expected_tip: &str,
    reason: &str,
    stopped: bool,
) -> Result<()> {
    ensure!(stopped, "admission recovery requires --all-reviewers-stopped; stop all watchers and verify all reviewers stopped first");
    ensure!(
        !reason.trim().is_empty() && reason.len() <= 4096,
        "admission recovery requires a bounded reason"
    );
    change_path(key)?;
    let hub = Hub::load(root)?.context("admission recovery requires an explicit hub")?;
    let store = store(root).context("admission recovery requires a canonical store")?;
    let config =
        publisher(root)?.context("admission recovery requires an enrolled local publisher")?;
    let repo = private_repo()?;
    let (tip, _) = read_table(repo.path(), &hub, key)?;
    ensure!(
        tip.as_deref() == Some(expected_tip),
        "admission recovery tip changed: expected {expected_tip}, observed {tip:?}; inspect again"
    );
    let roster = roster(&store, &super::head(&store)?)?;
    let id = Uuid::new_v4();
    let record = sign(
        &config,
        &serde_json::json!({
            "version": 1, "kind": "review-admission-recovery", "id": id,
            "node": config.node, "repository": hub.repository, "hub": hub.url,
            "reference": hub.reference, "change": key, "predecessor_tip": expected_tip,
            "all_reviewers_stopped": true, "reason": reason,
        }),
    )?;
    verify_signature(&config.node, &record, &roster)?;
    // Record the signed attestation in the same tree as the empty new anchor.
    git(repo.path(), &["read-tree", expected_tip])?;
    let path = repo.path().join("recovery.json");
    std::fs::write(&path, serde_json::to_vec(&record)?)?;
    let blob = text(git(
        repo.path(),
        &["hash-object", "-w", path.to_str().context("recovery path")?],
    )?)?;
    let rel = format!("recoveries/{id}.json");
    git(
        repo.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            "100644",
            &blob,
            &rel,
        ],
    )?;
    // commit_table reads the parent tree; first commit the audit as a private
    // parent, then commit the anchor. Neither is published unless the CAS wins.
    let tree = text(git(repo.path(), &["write-tree"])?)?;
    let audit = text(git(
        repo.path(),
        &[
            "-c",
            "user.name=AIDA review admission",
            "-c",
            "user.email=aida@localhost",
            "commit-tree",
            &tree,
            "-p",
            expected_tip,
            "-m",
            "review: operator recovery attestation",
        ],
    )?)?;
    let next = commit_table(
        repo.path(),
        Some(&audit),
        key,
        &Table {
            version: 1,
            anchor: Some(record.clone()),
            ..Table::default()
        },
    )?;
    require_pinned(root, &Some(hub.clone()))?;
    let current = super::roster(&store, &super::head(&store)?)?;
    ensure!(
        current.nodes == roster.nodes,
        "admission roster changed; inspect again"
    );
    remote_git(
        repo.path(),
        &hub.url,
        &[
            "push",
            "--porcelain",
            &format!("--force-with-lease={}:{expected_tip}", hub.reference),
            &hub.url,
            &format!("{next}:{}", hub.reference),
        ],
    )
    .context("admission recovery push uncertain; inspect hub before retrying")?;
    println!("{key}: admission re-anchored; audit {id}; old tokens cannot transition; reset stopped local watcher attempts before restarting");
    Ok(())
}

/// A local release never erases an active remote claim. After operator
/// re-anchoring, each node can clear its own stopped local receipts normally.
pub(crate) fn require_released(root: &Path, key: &str) -> Result<()> {
    let Some(hub) = Hub::load(root)? else {
        return Ok(());
    };
    let store = store(root).context("review admission needs a canonical store")?;
    let roster = roster(&store, &super::head(&store)?)?;
    let repo = private_repo()?;
    let (_, table) = read_table(repo.path(), &hub, key)?;
    validate_anchor(&hub, key, &table, &roster)?;
    let history = chain(
        &hub,
        key,
        table.changes.get(key).map(Vec::as_slice).unwrap_or(&[]),
        &roster,
    )?;
    ensure!(!history.last().is_some_and(|t| active(&t.state)),
        "distributed admission is active; stop watchers/reviewers and use review nodes admission-reconcile before local --release");
    Ok(())
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
        let fetched = remote_git(
            &store,
            "origin",
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
