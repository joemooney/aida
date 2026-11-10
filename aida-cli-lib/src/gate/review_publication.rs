//! Trusted-node review publications. Only the host recorder/collector issues
//! these after validating local authority. No grant or human receipt travels.
//! Trust comes from the CURRENT committed roster, never the publication itself.
//!
//! Layout (all paths relative to the canonical store):
//! - `reviews/v1/nodes.json`: the operator-enrolled node roster.
//! - `reviews/v1/changes/<CHANGE>/publications/<node>/<uuid>.json`: signed
//!   review receipts for one change.
//! - `reviews/v1/changes/<CHANGE>/authorship/<node>/<uuid>.json`: signed
//!   author-session evidence for one change.
//! - `reviews/v1/specs/<spec key>/<CHANGE>`: an unsigned routing marker naming
//!   a change a receipt covered; it confers nothing, the receipt still verifies.
//!
//! Every read is scoped to one change, so one malformed or oversized change
//! never refuses a different change or a spec reviewed only locally.
// trace:STORY-2492 | ai:codex
// trace:STORY-2492 | ai:claude
// mandate-gate
use super::ledger;
use super::{
    canon::{Snapshot, SpecKey},
    ledger::ChangeKey,
    review_authority as authority, review_verdict as rv,
};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use uuid::Uuid;

pub(crate) mod admission;

/// Verified synced history is consulted even without any local verdict files.
// trace:STORY-2492 | ai:codex
pub(crate) fn watcher_history(root: &Path, change: &str) -> Result<Vec<(String, String)>> {
    let Some((store, tip)) = store_view(root)? else {
        return Ok(Vec::new());
    };
    let key = ChangeKey::parse(change).context("invalid watcher change")?;
    let heads: std::collections::BTreeSet<_> = load_receipts(root, &store, &tip, change)?
        .into_iter()
        .map(|(receipt, ..)| receipt.head)
        .collect();
    let mut history = Vec::new();
    let started = std::time::Instant::now();
    for head in heads {
        ensure!(
            started.elapsed() < Duration::from_secs(20),
            "watcher history deadline exhausted"
        );
        if let Some((receipt, rel)) = select(root, key, &[], Some(&head))? {
            history.push((body(&receipt)?, format!(".aida-store/{rel}")));
        }
    }
    ensure!(
        head(&store)? == tip,
        "review history changed during watcher selection"
    );
    Ok(history)
}

const ROSTER: &str = "reviews/v1/nodes.json";
const CHANGES: &str = "reviews/v1/changes";
const SPEC_INDEX: &str = "reviews/v1/specs";
const PUBLICATIONS: &str = "publications";
const AUTHORSHIPS: &str = "authorship";
const NAMESPACE: &str = "aida-review-receipt";
const CONFIG: &str = ".aida/review-publisher.json";
const CAP: usize = 1024 * 1024;
/// Per-change read budget. A change that exceeds it refuses only itself.
const MAX_PER_CHANGE: usize = 256;

/// How many unread reports [`report`] keeps for [`take_reports`].
const REPORT_BACKLOG: usize = 64;

thread_local! {
    static REPORTS: std::cell::RefCell<std::collections::VecDeque<String>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

/// Emit an operator-facing publication report (AC3 disagreement, evidence
/// treated as absent) on stderr and keep it for [`take_reports`], so gate
/// paths that only return a verdict still leave a checkable report.
// trace:STORY-2492 | ai:claude
pub(crate) fn report(message: String) {
    eprintln!("{message}");
    REPORTS.with(|reports| {
        let mut reports = reports.borrow_mut();
        if reports.len() == REPORT_BACKLOG {
            reports.pop_front();
        }
        reports.push_back(message);
    });
}

/// The reports [`report`] emitted on this thread since the last call.
pub(crate) fn take_reports() -> Vec<String> {
    REPORTS.with(|reports| reports.borrow_mut().drain(..).collect())
}

#[derive(Debug, clap::Subcommand)]
pub enum NodeCommand {
    /// Re-anchor one admission chain after stopping all watchers and reviewers.
    AdmissionReconcile {
        /// GitHub change key, for example PR-123.
        change: String,
        /// Exact hub tip inspected by the operator (no automatic retries).
        #[arg(long)]
        expected_tip: String,
        /// Durable explanation of the stopped-admission recovery.
        #[arg(long)]
        reason: String,
        /// Attest that all watchers and all reviewers for this change are stopped.
        #[arg(long)]
        all_reviewers_stopped: bool,
    },
    /// Enroll a node's public ed25519 key in the shared roster.
    Enroll {
        /// Stable publishing node ID (letters, digits, hyphens and underscores).
        node: String,
        /// File containing the node's ed25519 public key.
        public_key: PathBuf,
    },
    /// Remove a node; all of its publications immediately lose credit after sync.
    Revoke {
        /// Publishing node to remove from the shared roster.
        node: String,
    },
    /// Configure this host recorder's node and private key path. Keys stay local.
    Publisher {
        /// This host's enrolled publishing node ID.
        node: String,
        /// Path to the local ed25519 signing key.
        private_key: PathBuf,
    },
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Roster {
    version: u32,
    nodes: BTreeMap<String, String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Publisher {
    node: String,
    private_key: PathBuf,
}

/// Fixed field order is the v1 canonical JSON encoding. The detached sshsig
/// signs precisely these bytes. Unknown fields and noncanonical bytes refuse.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    node: String,
    publication: Uuid,
    supersedes: Vec<Uuid>,
    repository: String,
    change: String,
    head: String,
    specs: Vec<SpecKey>,
    blocking_finding_origin_ids: Vec<String>,
    authors: Vec<Vec<String>>,
    reviewer_actor: String,
    reviewer_identity: Vec<String>,
    decision: String,
    recorded_at: String,
    branch: Option<String>,
    summary: Option<String>,
    findings: Vec<String>,
    // trace:STORY-2698 | ai:codex — absent preserves existing canonical signed bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    finding_dispositions: Option<Vec<serde_json::Value>>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Signed {
    payload: String,
    sshsig: String,
}

fn node_valid(node: &str) -> bool {
    !node.is_empty()
        && node.len() <= 128
        && node
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn run(mut cmd: Command) -> Result<Vec<u8>> {
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    let out = super::kernel_deps::bounded_review_output(cmd, Duration::from_secs(10), CAP as u64)
        .context("review publication command failed or timed out")?;
    ensure!(
        out.status.success(),
        "review publication command refused: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    ensure!(
        out.stdout.len() <= CAP,
        "review publication command exceeded output budget"
    );
    Ok(out.stdout)
}
fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut cmd = Command::new("git");
    cmd.current_dir(root).args(args);
    run(cmd)
}
fn text(bytes: Vec<u8>) -> Result<String> {
    Ok(String::from_utf8(bytes)?.trim().to_string())
}
fn store(root: &Path) -> Option<PathBuf> {
    aida_core::store_locate::detect_distributed_store_from(root)
}
fn head(store: &Path) -> Result<String> {
    text(git(store, &["rev-parse", "--verify", "HEAD^{commit}"])?)
}
fn blob(store: &Path, head: &str, rel: &str) -> Result<Option<Vec<u8>>> {
    let listing = git(store, &["ls-tree", head, "--", rel])?;
    if listing.is_empty() {
        return Ok(None);
    }
    ensure!(
        listing.starts_with(b"100644 blob "),
        "review evidence is not a regular committed blob: {rel}"
    );
    Ok(Some(git(store, &["show", &format!("{head}:{rel}")])?))
}
fn roster(store: &Path, head: &str) -> Result<Roster> {
    let Some(bytes) = blob(store, head, ROSTER)? else {
        return Ok(Roster {
            version: 1,
            ..Default::default()
        });
    };
    let value: Roster = serde_json::from_slice(&bytes)?;
    ensure!(
        value.version == 1 && value.nodes.len() <= 256,
        "unsupported or oversized review node roster"
    );
    for (node, key) in &value.nodes {
        ensure!(
            node_valid(node) && public_key_valid(key),
            "invalid review node roster entry"
        );
    }
    Ok(value)
}
fn public_key_valid(key: &str) -> bool {
    let parts: Vec<_> = key.split_whitespace().collect();
    parts.len() == 2
        && parts[0] == "ssh-ed25519"
        && parts[1].len() < 256
        && parts[1]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
}
fn repository(root: &Path) -> Result<String> {
    // Hash the exact configured origin: no embedded credentials enter the store.
    // Alternate URL spellings require the operator to align origins.
    use sha2::{Digest, Sha256};
    let origin = git(root, &["remote", "get-url", "origin"])?;
    Ok(format!("{:x}", Sha256::digest(origin)))
}

fn evidence_dir(change: &str, kind: &str) -> String {
    format!("{CHANGES}/{change}/{kind}")
}
fn evidence_path(change: &str, kind: &str, node: &str, id: Uuid) -> String {
    format!("{}/{node}/{id}.json", evidence_dir(change, kind))
}
/// Committed names under `dir`, bounded per change.
fn list(store: &Path, tip: &str, dir: &str) -> Result<Vec<String>> {
    let names = String::from_utf8(git(
        store,
        &["ls-tree", "-r", "--name-only", tip, "--", dir],
    )?)?;
    let names: Vec<String> = names.lines().map(str::to_string).collect();
    ensure!(
        names.len() <= MAX_PER_CHANGE,
        "review evidence read budget exhausted for {dir}"
    );
    Ok(names)
}

/// The configured publisher of this host, if it opted in.
fn publisher(root: &Path) -> Result<Option<Publisher>> {
    match std::fs::read(root.join(CONFIG)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice::<Publisher>(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Preserve concurrent publications. A rejected exact push of immutable
/// evidence rebases it on the winner, inside the store writer lock (the
/// lock is reentrant, and the pull takes the sync lock beneath it), so no
/// other local store writer interleaves between the rebase and the check.
/// No snapshot rollback/reset is ever performed.
fn persist(store: &Path, files: &[(String, Vec<u8>)], immutable: bool) -> Result<()> {
    persist_checked(store, files, immutable, None)
}
fn persist_checked(
    store: &Path,
    files: &[(String, Vec<u8>)],
    immutable: bool,
    expected_head: Option<&str>,
) -> Result<()> {
    use aida_core::{db::with_store_write_lock, git_ops};
    // This new store writer has no enabled-WORK prepared transport adapter.
    aida_core::transport_policy::refuse_unsupported(store)?;
    let rels: Vec<&str> = files.iter().map(|(rel, _)| rel.as_str()).collect();
    let (mut tip, branch) = with_store_write_lock(store, || -> Result<(String, String)> {
        git_ops::ensure_store_write_safe(store)?;
        ensure!(
            expected_head.is_none_or(|expected| head(store).is_ok_and(|now| now == expected)),
            "roster changed locally; reapply the operator edit"
        );
        ensure!(
            git(store, &["diff", "--cached", "--name-only"])?.is_empty(),
            "store has unrelated staged changes"
        );
        let mut args = vec!["diff", "--name-only", "--"];
        args.extend(rels.iter().copied());
        ensure!(
            git(store, &args)?.is_empty(),
            "review evidence has uncommitted edits"
        );
        for (rel, bytes) in files {
            let path = store.join(rel);
            if immutable && path.exists() {
                ensure!(
                    std::fs::read(&path)? == *bytes,
                    "immutable review publication collision"
                );
                continue;
            }
            let parent = path.parent().context("review publication parent")?;
            std::fs::create_dir_all(parent)?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            use std::io::Write;
            file.write_all(bytes)?;
            file.as_file().sync_all()?;
            file.persist(&path)?;
        }
        git_ops::add(store, &rels)?;
        git_ops::commit(store, "review: publish trusted node evidence")?;
        Ok((git_ops::head_sha(store)?, git_ops::current_branch(store)?))
    })?;
    if !git_ops::has_remote(store, "origin") {
        return Ok(());
    }
    for _ in 0..3 {
        if git_ops::push_exact(store, "origin", &tip, &branch)? {
            return Ok(());
        }
        ensure!(
            immutable,
            "the roster edit is committed locally ({tip}) but its push raced another \
             operator. Run `aida sync` to reconcile the store, then rerun the same \
             command: enroll/revoke is idempotent and only pushes when the roster \
             already holds the edit. If the sync reports a roster conflict, compare \
             `git -C {} log origin/{branch}..HEAD` with the shared roster and drop \
             the local roster commit before reapplying",
            store.display()
        );
        // Distinct immutable paths rebase cleanly. Never structurally auto-merge
        // a conflicting pending operator roster edit: that could undo revocation.
        tip = with_store_write_lock(store, || -> Result<String> {
            git_ops::ensure_store_write_safe(store)?;
            git_ops::pull_rebase(store, "origin", &branch)?;
            let tip = head(store)?;
            for (rel, bytes) in files {
                ensure!(
                    blob(store, &tip, rel)?.as_deref() == Some(bytes.as_slice()),
                    "review publication changed during synchronization"
                );
            }
            Ok(tip)
        })?;
    }
    bail!("review publication remains local pending sync: push rejected")
}

/// Push an already-committed local roster edit (the idempotent rerun path).
fn push_pending_roster(store: &Path) -> Result<()> {
    use aida_core::git_ops;
    if !git_ops::has_remote(store, "origin") {
        return Ok(());
    }
    let branch = git_ops::current_branch(store)?;
    let tip = head(store)?;
    ensure!(
        git_ops::push_exact(store, "origin", &tip, &branch)?,
        "the roster already holds this edit locally, but the push was rejected; run \
         `aida sync` first, then rerun the command"
    );
    Ok(())
}

pub(crate) fn nodes(command: &NodeCommand) -> Result<()> {
    authority_operator()?;
    let root = std::env::current_dir()?;
    let root = super::ledger::store_root(&root)?;
    if let NodeCommand::AdmissionReconcile {
        change,
        expected_tip,
        reason,
        all_reviewers_stopped,
    } = command
    {
        return admission::reconcile(&root, change, expected_tip, reason, *all_reviewers_stopped);
    }
    let store = store(&root).context("review nodes require a distributed store")?;
    let (tip, mut roster) = aida_core::db::with_store_write_lock(&store, || -> Result<_> {
        let tip = head(&store)?;
        Ok((tip.clone(), roster(&store, &tip)?))
    })?;
    match command {
        NodeCommand::AdmissionReconcile { .. } => unreachable!(),
        NodeCommand::Enroll { node, public_key } => {
            ensure!(node_valid(node), "invalid publishing node id");
            let raw = std::fs::read_to_string(public_key)?;
            let key = raw.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
            ensure!(public_key_valid(&key), "expected an ed25519 public key");
            let tmp = tempfile::NamedTempFile::new()?;
            std::fs::write(tmp.path(), &key)?;
            let mut cmd = Command::new("ssh-keygen");
            cmd.args(["-l", "-f"]).arg(tmp.path());
            run(cmd)?;
            match roster.nodes.get(node) {
                // Rerun after a raced push: the edit is already committed.
                Some(existing) if *existing == key => return push_pending_roster(&store),
                Some(_) => bail!("node already enrolled with another key; revoke it first"),
                None => {}
            }
            roster.nodes.insert(node.clone(), key);
            persist_checked(
                &store,
                &[(ROSTER.into(), serde_json::to_vec(&roster)?)],
                false,
                Some(&tip),
            )
        }
        NodeCommand::Revoke { node } => {
            if roster.nodes.remove(node).is_none() {
                return push_pending_roster(&store);
            }
            persist_checked(
                &store,
                &[(ROSTER.into(), serde_json::to_vec(&roster)?)],
                false,
                Some(&tip),
            )
        }
        NodeCommand::Publisher { node, private_key } => {
            ensure!(
                roster.nodes.contains_key(node),
                "enroll the publishing node first"
            );
            let config = Publisher {
                node: node.clone(),
                private_key: private_key.canonicalize()?,
            };
            std::fs::create_dir_all(root.join(".aida"))?;
            std::fs::write(root.join(CONFIG), serde_json::to_vec(&config)?)?;
            Ok(())
        }
    }
}

fn authority_operator() -> Result<()> {
    super::seat_authority::require_direct_human()
}

fn qualify(node: &str, identity: &[String]) -> Vec<String> {
    identity
        .iter()
        .filter_map(|token| {
            let (kind, value) = token.split_once(':')?;
            match kind {
                "session" => Some(token.clone()), // sessions survive resumption across nodes
                "lease" | "proc" | "shell" | "sid" => Some(format!("{kind}:{node}/{value}")),
                _ => None, // Never export key possession/grant binding tokens.
            }
        })
        .collect()
}
fn verify(receipt: &Receipt, signed: &Signed, roster: &Roster) -> Result<()> {
    ensure!(
        receipt.version == 1 && node_valid(&receipt.node),
        "unsupported review receipt"
    );
    ensure!(
        serde_json::to_string(receipt)? == signed.payload,
        "noncanonical review receipt"
    );
    ensure!(
        receipt.head.len() == 40 && receipt.head.bytes().all(|b| b.is_ascii_hexdigit()),
        "review receipt needs exact full head"
    );
    ensure!(
        ChangeKey::parse(&receipt.change).is_some_and(|c| c.as_key() == receipt.change)
            && !receipt.specs.is_empty(),
        "unbound review receipt"
    );
    ensure!(
        !receipt.reviewer_actor.is_empty()
            && authority::has_identity_anchor(&receipt.reviewer_identity),
        "missing reviewer identity"
    );
    ensure!(
        matches!(
            receipt.decision.as_str(),
            "approved" | "request-changes" | "rejected" | "waiting"
        ),
        "unsupported receipt decision"
    );
    ensure!(
        !receipt.authors.is_empty()
            && receipt
                .authors
                .iter()
                .all(|a| a.iter().any(|t| t.starts_with("session:"))),
        "missing portable author identity evidence"
    );
    ensure!(
        receipt
            .authors
            .iter()
            .all(|author| !authority::identities_overlap(author, &receipt.reviewer_identity)),
        "reviewer is an author across nodes"
    );
    ensure!(
        !receipt.supersedes.contains(&receipt.publication),
        "publication supersedes itself"
    );
    verify_signature(&receipt.node, signed, roster)
}
fn verify_signature(node: &str, signed: &Signed, roster: &Roster) -> Result<()> {
    ensure!(node_valid(node), "invalid publishing node");
    let key = roster
        .nodes
        .get(node)
        .context("review publishing node is not enrolled (or was revoked)")?;
    let temp = tempfile::tempdir()?;
    let allowed = temp.path().join("allowed_signers");
    let sig = temp.path().join("receipt.sig");
    let payload = temp.path().join("receipt.json");
    std::fs::write(&allowed, format!("{node} {key}\n"))?;
    std::fs::write(&sig, &signed.sshsig)?;
    std::fs::write(&payload, &signed.payload)?;
    let mut cmd = Command::new("ssh-keygen");
    cmd.args(["-Y", "verify", "-f"])
        .arg(allowed)
        .args(["-I", node, "-n", NAMESPACE, "-s"])
        .arg(sig)
        .stdin(Stdio::from(std::fs::File::open(payload)?));
    run(cmd)?;
    Ok(())
}
fn sign(config: &Publisher, value: &impl Serialize) -> Result<Signed> {
    let payload = serde_json::to_string(value)?;
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("receipt.json");
    std::fs::write(&path, &payload)?;
    let mut cmd = Command::new("ssh-keygen");
    cmd.args(["-Y", "sign", "-f"])
        .arg(&config.private_key)
        .args(["-n", NAMESPACE])
        .arg(&path)
        .stdin(Stdio::null());
    run(cmd)?;
    Ok(Signed {
        payload,
        sshsig: std::fs::read_to_string(path.with_extension("json.sig"))?,
    })
}

/// The recorder/collector hook: the verdict is already durably on the local
/// ledger, so a publication that cannot be issued is a reported pending
/// state with its retry command, never a failure of the recorded verdict.
pub(crate) fn publish_reported(
    root: &Path,
    change: ChangeKey,
    record: &rv::JsonObj,
    labels: &[String],
) {
    if let Err(e) = publish(root, change, record, labels) {
        eprintln!(
            "warning: the verdict for {} is recorded locally, but its review publication \
             is pending: {e:#}\n  retry: aida review publish --pr {} --specs {} \
             [--blocking-findings <filed finding IDs>]",
            change.as_key(),
            change.number,
            if labels.is_empty() {
                "<SPEC-IDS>".to_string()
            } else {
                labels.join(",")
            }
        );
    }
}

/// Called only on the host writer path. A configuration opts that node into
/// publication. Revalidate the ordinary local authority before issuing a receipt.
pub(crate) fn publish(
    root: &Path,
    change: ChangeKey,
    record: &rv::JsonObj,
    labels: &[String],
) -> Result<()> {
    let Some(config) = publisher(root)? else {
        return Ok(());
    };
    ensure!(
        !super::ledger::contained(),
        "contained callers cannot publish review receipts"
    );
    let store = store(root).context("publisher requires distributed store")?;
    let verdict = rv::parse_recorded_verdict(&serde_json::to_string(record)?)
        .context("invalid review verdict")?;
    // Lifecycle closure is not a new act of review and must not republish approval.
    if verdict.is_closed() {
        return Ok(());
    }
    let verified = authority::verified_authority_keys(root, std::slice::from_ref(&verdict));
    let att = verdict
        .attestation
        .as_ref()
        .context("publication requires validated recorder")?;
    ensure!(
        att.receipt_id
            .as_deref()
            .is_none_or(|id| !id.starts_with("publication:")),
        "cannot republish remote authority"
    );
    ensure!(
        att.authority_key(verdict.recorded_at.as_deref())
            .is_some_and(|k| verified.contains_key(&k)),
        "local review authority did not validate"
    );
    ensure!(
        att.authorizes(verdict.reviewed_sha.as_deref()),
        "recorder is not authorized to review this head"
    );
    let snapshot = Snapshot::at_head(root)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .context("missing canonical snapshot")?;
    // The receipt covers the change's bound spec set (its signed authorship
    // coverage), whichever one spec, or none, the recorder named.
    let (authors, bound) = author_evidence(root, change)?;
    let requested = snapshot.resolve(labels);
    ensure!(
        requested.unresolved.is_empty() && requested.ambiguous.is_empty(),
        "unresolved publication specs"
    );
    ensure!(
        requested.members.iter().all(|m| bound.contains(&m.key)),
        "review names a spec {} was not opened for",
        change.as_key()
    );
    let bound_labels = spec_labels(&snapshot, &bound)?;
    if verdict.kind.approves() {
        ensure!(
            super::review_approval::independent_approval_authorized(
                root,
                &bound_labels,
                verdict.reviewed_branch.as_deref(),
                &verdict
            ),
            "ordinary local approval validation refused publication"
        );
    }
    let mut authenticated_identity = verified
        .get(
            &att.authority_key(verdict.recorded_at.as_deref())
                .context("missing authority key")?,
        )
        .context("missing authenticated reviewer identity")?
        .clone();
    authenticated_identity.extend(att.identity.iter().cloned());
    authenticated_identity.sort();
    authenticated_identity.dedup();
    let mut origins = Vec::new();
    if let Some(ids) = record.get("blocking_finding_origin_ids") {
        for id in ids.as_array().context("finding origins must be an array")? {
            let id = id.as_str().context("finding origin must be a string")?;
            let resolved = snapshot.resolve(&[id]);
            let item = resolved
                .members
                .first()
                .context("unresolved blocking finding origin")?;
            ensure!(item.native == id, "blocking finding must use its origin ID");
            origins.push(id.to_string());
        }
    }
    // An ordinary refusal (`aida review record --finding <TEXT>`, or a
    // collected reviewer verdict) carries its findings as signed prose; filed
    // finding origin IDs are optional and, when named, must resolve. A
    // refusal confers no authority, so it never waits on filing.
    // trace:STORY-2492 | ai:claude
    origins.sort();
    origins.dedup();
    let tip = head(&store)?;
    let key = change.as_key();
    let spec_keys: Vec<SpecKey> = bound;
    let receipt = Receipt {
        version: 1,
        node: config.node.clone(),
        publication: Uuid::new_v4(),
        supersedes: load_receipts(root, &store, &tip, &key)?
            .iter()
            .filter(|(r, ..)| r.node == config.node)
            .map(|(r, ..)| r.publication)
            .collect(),
        repository: repository(root)?,
        change: key.clone(),
        head: verdict
            .reviewed_sha
            .clone()
            .context("missing exact review head")?,
        specs: spec_keys.clone(),
        blocking_finding_origin_ids: origins,
        authors,
        reviewer_actor: verdict
            .recorded_by
            .clone()
            .context("missing reviewer actor")?,
        reviewer_identity: qualify(&config.node, &authenticated_identity),
        decision: record
            .get("verdict")
            .and_then(|v| v.as_str())
            .context("missing verdict")?
            .to_string(),
        recorded_at: verdict.recorded_at.clone().context("missing review time")?,
        branch: verdict.reviewed_branch.clone(),
        summary: verdict.summary.clone(),
        findings: verdict.findings.clone(),
        finding_dispositions: record
            .get("finding_dispositions")
            .and_then(|v| serde_json::from_value(v.clone()).ok()),
    };
    ensure!(
        receipt
            .authors
            .iter()
            .all(|tokens| !authority::identities_overlap(tokens, &receipt.reviewer_identity)),
        "reviewer is an author across nodes"
    );
    let signed = sign(&config, &receipt)?;
    verify(&receipt, &signed, &roster(&store, &tip)?)?;
    let mut files = vec![(
        evidence_path(&key, PUBLICATIONS, &receipt.node, receipt.publication),
        serde_json::to_vec(&signed)?,
    )];
    // Spec routing markers: a spec-keyed reader finds its changes without
    // reading any other change's evidence.
    for spec in &receipt.specs {
        files.push((
            format!("{SPEC_INDEX}/{spec}/{key}"),
            key.clone().into_bytes(),
        ));
    }
    persist(&store, &files, true)
}

/// Every committed receipt of one change, parsed and identity-checked (not
/// yet signature-verified). An unreadable blob refuses this change only.
fn load_receipts(
    root: &Path,
    store: &Path,
    tip: &str,
    change: &str,
) -> Result<Vec<(Receipt, Signed, String)>> {
    let names = list(store, tip, &evidence_dir(change, PUBLICATIONS))?;
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let repository = repository(root)?;
    let mut out = Vec::new();
    let mut total = 0;
    let started = std::time::Instant::now();
    for name in names {
        ensure!(
            started.elapsed() < Duration::from_secs(20),
            "publication read deadline exhausted"
        );
        let bytes = blob(store, tip, &name)?.context("publication vanished")?;
        total += bytes.len();
        ensure!(
            total <= 8 * CAP,
            "review publication aggregate budget exhausted"
        );
        let signed: Signed = serde_json::from_slice(&bytes)
            .with_context(|| format!("malformed review publication {name}"))?;
        let receipt: Receipt = serde_json::from_str(&signed.payload)
            .with_context(|| format!("malformed review receipt {name}"))?;
        ensure!(
            receipt.change == change
                && name == evidence_path(change, PUBLICATIONS, &receipt.node, receipt.publication),
            "publication identity/path mismatch"
        );
        if receipt.repository != repository {
            continue;
        }
        out.push((receipt, signed, name));
    }
    Ok(out)
}

/// A local store directory that is not a Git repository has no publication
/// authority; an attempted review namespace or explicit publisher there still
/// goes through strict refusal.
fn store_view(root: &Path) -> Result<Option<(PathBuf, String)>> {
    let Some(store) = store(root) else {
        return Ok(None);
    };
    let absent = |path: &Path| matches!(std::fs::symlink_metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound);
    if absent(&store.join(".git"))
        && absent(&store.join("HEAD"))
        && absent(&store.join("reviews"))
        && absent(&root.join(CONFIG))
    {
        return Ok(None);
    }
    let tip = head(&store)?;
    Ok(Some((store, tip)))
}

fn reference(r: &Receipt) -> String {
    format!("publication:{}:{}:{}", r.change, r.node, r.publication)
}
fn parse_reference(id: &str) -> Option<(String, String, Uuid)> {
    let mut parts = id.strip_prefix("publication:")?.splitn(3, ':');
    let change = ChangeKey::parse(parts.next()?)?.as_key();
    let node = parts.next().filter(|n| node_valid(n))?.to_string();
    let id = Uuid::parse_str(parts.next()?).ok()?;
    Some((change, node, id))
}
fn body(r: &Receipt) -> Result<String> {
    let att = rv::RecorderAttestation {
        sha: r.head.clone(),
        seat: Some("review-publication".into()),
        human_at_tty: false,
        receipt_id: Some(reference(r)),
        identity: r.reviewer_identity.clone(),
        ..Default::default()
    };
    let mut projected = serde_json::json!({
        "verdict": r.decision, "reviewed_sha": r.head, "reviewed_branch": r.branch,
        "recorded_at": r.recorded_at, "recorded_by": r.reviewer_actor,
        "summary": r.summary, "findings": r.findings,
        "blocking_finding_origin_ids": r.blocking_finding_origin_ids,
        "recorder_attestation": att,
    });
    // trace:STORY-2698 | ai:codex — absent metadata must stay absent on read-through.
    if let Some(dispositions) = &r.finding_dispositions {
        projected["finding_dispositions"] = serde_json::to_value(dispositions)?;
    }
    Ok(serde_json::to_string(&projected)?)
}

/// The change's current head as this node's code checkout knows it: the tip
/// of the reviewed branch (`origin/<branch>`, else the local branch). When no
/// branch resolves, publications that all name one head answer; several
/// heads with no resolvable branch cannot be ordered (`None`).
fn current_head(root: &Path, receipts: &[&(Receipt, Signed, String)]) -> Option<String> {
    let mut tips = std::collections::BTreeSet::new();
    for branch in receipts.iter().filter_map(|(r, ..)| r.branch.as_deref()) {
        if let Some(tip) = branch_tip(root, branch) {
            tips.insert(tip);
        }
    }
    if tips.len() == 1 {
        return tips.into_iter().next();
    }
    let heads: std::collections::BTreeSet<String> = receipts
        .iter()
        .map(|(r, ..)| r.head.to_ascii_lowercase())
        .collect();
    if heads.len() == 1 {
        heads.into_iter().next()
    } else {
        None
    }
}
fn branch_tip(root: &Path, branch: &str) -> Option<String> {
    if branch.is_empty() || branch.starts_with('-') || branch.contains("..") {
        return None;
    }
    ["refs/remotes/origin/", "refs/heads/"]
        .iter()
        .find_map(|prefix| {
            git(
                root,
                &[
                    "rev-parse",
                    "--verify",
                    "-q",
                    &format!("{prefix}{branch}^{{commit}}"),
                ],
            )
            .ok()
            .and_then(|out| text(out).ok())
            .filter(|sha| sha.len() == 40)
            .map(|sha| sha.to_ascii_lowercase())
        })
}

/// The current publication of ONE change: verified, unrevoked, causal
/// supersession applied per node, and restricted to the change's current head
/// (`expected_head`, else [`current_head`]). Publications at another head are
/// stale and never block or credit this head. One current answer per node;
/// a refusal from any node at this head wins over approvals.
fn select(
    root: &Path,
    change: ChangeKey,
    spec_ids: &[&str],
    expected_head: Option<&str>,
) -> Result<Option<(Receipt, String)>> {
    let Some((store, tip)) = store_view(root)? else {
        return Ok(None);
    };
    let key = change.as_key();
    let all = load_receipts(root, &store, &tip, &key)?;
    if all.is_empty() {
        return Ok(None);
    }
    let roster = roster(&store, &tip)?;
    let mut relevant: Vec<_> = all
        .iter()
        .filter(|(r, ..)| roster.nodes.contains_key(&r.node))
        .collect();
    if relevant.is_empty() {
        // Revocation removes the publisher's evidence, it does not lock the
        // change: a fresh review (local, or from an enrolled node) answers.
        report(format!(
            "review publications of {key} are ignored: every publishing node was revoked; \
             a fresh review of the change answers for it"
        ));
        return Ok(None);
    }
    let started = std::time::Instant::now();
    for (r, signed, _) in &relevant {
        ensure!(
            started.elapsed() < Duration::from_secs(20),
            "review verification deadline exhausted"
        );
        verify(r, signed, &roster)?;
    }
    // Supersession is signed and scoped to one publisher of this change.
    let mut pending: Vec<_> = relevant
        .iter()
        .map(|(r, ..)| (r.node.clone(), r.publication))
        .collect();
    while !pending.is_empty() {
        let before = pending.len();
        let ready: Vec<_> = pending
            .iter()
            .filter(|(node, id)| {
                relevant
                    .iter()
                    .find(|(r, ..)| r.node == *node && r.publication == *id)
                    .is_some_and(|(r, ..)| {
                        !r.supersedes
                            .iter()
                            .any(|old| pending.contains(&(node.clone(), *old)))
                    })
            })
            .cloned()
            .collect();
        pending.retain(|id| !ready.contains(id));
        ensure!(
            pending.len() < before,
            "cyclic review publication supersession"
        );
    }
    let superseded: Vec<_> = relevant
        .iter()
        .flat_map(|(r, ..)| r.supersedes.iter().map(|id| (r.node.clone(), *id)))
        .collect();
    relevant.retain(|(r, ..)| !superseded.contains(&(r.node.clone(), r.publication)));
    ensure!(
        !relevant.is_empty(),
        "review publication selection lost all evidence"
    );
    let change_head = match expected_head {
        Some(h) => h.trim().to_ascii_lowercase(),
        None => match current_head(root, &relevant) {
            Some(h) => h,
            None => {
                // No head to order by: absent here, never a standing refusal.
                // Head-bound gates (merge, train) pass the head and still see
                // the publication at it.
                report(format!(
                    "review publications of {key} are ignored here: they name several heads \
                     and its branch is not fetched; fetch the branch to apply them"
                ));
                return Ok(None);
            }
        },
    };
    relevant.retain(|(r, ..)| r.head.eq_ignore_ascii_case(&change_head));
    if relevant.is_empty() {
        // Every surviving publication reviewed another head: stale here.
        return Ok(None);
    }
    relevant.sort_by(|(a, ..), (b, ..)| (&a.node, a.publication).cmp(&(&b.node, b.publication)));
    let (receipt, _, rel) = relevant
        .iter()
        .find(|(r, ..)| r.decision != "approved")
        .copied()
        .unwrap_or(relevant[0]);
    let snapshot = Snapshot::at_head(root)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .context("missing canonical review snapshot")?;
    if receipt.decision == "approved" {
        let requested = snapshot.resolve(spec_ids);
        ensure!(
            requested.unresolved.is_empty()
                && requested.ambiguous.is_empty()
                && requested
                    .members
                    .iter()
                    .all(|member| receipt.specs.contains(&member.key)),
            "signed review publication does not cover every requested canonical spec"
        );
    }
    // Require finding identities to remain resolvable on the receiving node.
    for id in &receipt.blocking_finding_origin_ids {
        let set = snapshot.resolve(&[id]);
        ensure!(
            set.members.first().is_some_and(|m| m.native == *id),
            "synced blocking finding origin is missing"
        );
    }
    ensure!(
        head(&store)? == tip,
        "review roster changed during selection"
    );
    Ok(Some((receipt.clone(), rel.clone())))
}

/// The change a spec-keyed read selects: of the changes whose receipts
/// covered the spec, the newest (highest number). An older change's
/// refusal never answers for the spec once a newer change carries it.
fn spec_change(root: &Path, spec_ids: &[&str]) -> Result<Option<ChangeKey>> {
    let Some((store, tip)) = store_view(root)? else {
        return Ok(None);
    };
    if git(&store, &["ls-tree", &tip, "--", SPEC_INDEX])?.is_empty() {
        return Ok(None);
    }
    let snapshot = Snapshot::at_head(root)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .context("missing canonical review snapshot")?;
    let mut newest: Option<ChangeKey> = None;
    for member in snapshot.resolve(spec_ids).members {
        let dir = format!("{SPEC_INDEX}/{}", member.key);
        for name in list(&store, &tip, &dir)? {
            let Some(change) = name
                .rsplit('/')
                .next()
                .and_then(ChangeKey::parse)
                .filter(|c| name.ends_with(&c.as_key()))
            else {
                continue;
            };
            if newest.is_none_or(|n| change.number > n.number) {
                newest = Some(change);
            }
        }
    }
    Ok(newest)
}

/// Change ids select their own change (spec ids are then coverage the
/// approval must hold). Spec-only ids route through [`spec_change`].
fn current_for(
    root: &Path,
    ids: &[&str],
    expected_head: Option<&str>,
) -> Result<Option<rv::CurrentVerdict>> {
    let changes: Vec<ChangeKey> = ids.iter().filter_map(|id| ChangeKey::parse(id)).collect();
    let spec_ids: Vec<&str> = ids
        .iter()
        .copied()
        .filter(|id| ChangeKey::parse(id).is_none())
        .collect();
    let changes = if changes.is_empty() {
        spec_change(root, &spec_ids)?.into_iter().collect()
    } else {
        changes
    };
    let mut chosen: Option<(Receipt, String)> = None;
    for change in changes {
        if let Some(selected) = select(root, change, &spec_ids, expected_head)? {
            let blocks = selected.0.decision != "approved";
            if chosen.is_none() || blocks {
                chosen = Some(selected);
            }
            if blocks {
                break;
            }
        }
    }
    let Some((receipt, rel)) = chosen else {
        return Ok(None);
    };
    let store = store(root).context("missing review store")?;
    Ok(Some(rv::CurrentVerdict {
        body: body(&receipt)?,
        source: rv::VerdictSource::File(store.join(rel)),
        written_at: None,
    }))
}

/// The disagreement report between a local verdict and the synced selection
/// at the same head (AC3). Different heads are staleness, not disagreement.
pub(crate) fn disagreement(
    id: &str,
    local: &rv::RecordedVerdict,
    remote: &rv::RecordedVerdict,
) -> Option<String> {
    let (Some(local_head), Some(remote_head)) = (&local.reviewed_sha, &remote.reviewed_sha) else {
        return None;
    };
    if !local_head.eq_ignore_ascii_case(remote_head)
        || (local.kind == remote.kind && local.findings == remote.findings)
    {
        return None;
    }
    Some(format!(
        "review publication/local verdict disagreement for {id} at {}: local {:?}, synced \
         {:?}; using the synced evidence",
        &remote_head[..remote_head.len().min(12)],
        local.kind,
        remote.kind
    ))
}

/// Synced evidence wins. Invalid evidence becomes an explicit non-approval
/// for the affected change only; Option-shaped legacy readers must never fall
/// back to a local approval over a refused publication.
pub(crate) fn current(root: &Path, ids: &[&str]) -> Option<rv::CurrentVerdict> {
    resolve_current(root, ids, None)
}

pub(crate) fn current_at_head(root: &Path, ids: &[&str], head: &str) -> Option<rv::CurrentVerdict> {
    resolve_current(root, ids, Some(head))
}

fn resolve_current(root: &Path, ids: &[&str], head: Option<&str>) -> Option<rv::CurrentVerdict> {
    match current_for(root, ids, head) {
        Ok(value) => {
            if let Some(remote) = value
                .as_ref()
                .and_then(|v| rv::parse_recorded_verdict(&v.body))
            {
                for id in ids {
                    let local = super::ledger::project::newest_verdict(root, id)
                        .and_then(|record| rv::parse_ledger_record(&record));
                    if let Some(message) = local.and_then(|l| disagreement(id, &l, &remote)) {
                        report(message);
                    }
                }
            }
            value
        }
        Err(e) => {
            eprintln!("review publication refused: {e:#}");
            Some(rv::CurrentVerdict {
                body: serde_json::json!({"verdict":"request-changes", "summary":format!("review publication refused: {e:#}")}).to_string(),
                source: rv::VerdictSource::File(root.join(".aida-store").join(CHANGES)), written_at: None,
            })
        }
    }
}

/// Re-read signature and current roster at every authority boundary. This is
/// authority for precisely this publication; it is never a remote seat grant.
pub(crate) fn verified_identity(root: &Path, verdict: &rv::RecordedVerdict) -> Result<Vec<String>> {
    let att = verdict
        .attestation
        .as_ref()
        .context("missing publication attestation")?;
    let (change, node, id) = att
        .receipt_id
        .as_deref()
        .and_then(parse_reference)
        .context("missing publication reference")?;
    let (store, tip) = store_view(root)?.context("missing publication store")?;
    let rel = evidence_path(&change, PUBLICATIONS, &node, id);
    let signed: Signed =
        serde_json::from_slice(&blob(&store, &tip, &rel)?.context("publication not found")?)?;
    let receipt: Receipt = serde_json::from_str(&signed.payload)?;
    ensure!(
        receipt.change == change && receipt.node == node && receipt.publication == id,
        "publication identity/path mismatch"
    );
    ensure!(
        receipt.repository == repository(root)?,
        "publication belongs to another repository"
    );
    verify(&receipt, &signed, &roster(&store, &tip)?)?;
    let expected =
        rv::parse_recorded_verdict(&body(&receipt)?).context("invalid publication projection")?;
    ensure!(
        &expected == verdict,
        "verdict differs from its signed publication"
    );
    let snapshot = Snapshot::at_head(root)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .context("missing spec snapshot")?;
    let labels: Vec<String> = receipt
        .specs
        .iter()
        .map(|key| {
            snapshot
                .spec(*key)
                .map(|s| s.labels.clone())
                .context("publication spec disappeared")
        })
        .collect::<Result<Vec<Vec<String>>>>()?
        .into_iter()
        .flatten()
        .collect();
    let (ledger, error) = authority::load_ledger_checked(root);
    ensure!(error.is_none(), "authorship evidence is unreadable");
    let authors = authority::author_sessions(
        &authority::visible_leases(root),
        &ledger,
        &labels,
        receipt.branch.as_deref(),
        |_| None,
    );
    for author in authors {
        let tokens: Vec<_> = author
            .tokens
            .into_iter()
            .filter(|t| t.starts_with("session:"))
            .collect();
        ensure!(
            !authority::identities_overlap(&tokens, &receipt.reviewer_identity),
            "reviewer is a local author"
        );
    }
    ensure!(
        head(&store)? == tip,
        "review roster changed during authority check"
    );
    let change_key = ChangeKey::parse(&change).context("publication change")?;
    let (selected, _) = select(root, change_key, &[], Some(&receipt.head))?
        .context("publication is no longer current")?;
    ensure!(
        selected == receipt,
        "publication was superseded or opposed by a refusal"
    );
    Ok(receipt.reviewer_identity.clone())
}

/// Portable author identity asserted by the trusted host opening or pushing a
/// change. Contains only session identities, not grant/receipt/process
/// authority. Authors accumulate per change: every head pushed through AIDA
/// adds its author sessions, and a reviewer must differ from all of them.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Authorship {
    version: u32,
    node: String,
    publication: Uuid,
    repository: String,
    change: String,
    head: String,
    branch: Option<String>,
    specs: Vec<SpecKey>,
    authors: Vec<Vec<String>>,
}

/// Publish author sessions for `change` at exactly `head` (the opened or
/// pushed branch tip, never the caller checkout's HEAD).
pub(crate) fn publish_authorship(
    start: &Path,
    change: ChangeKey,
    labels: &[String],
    head_sha: &str,
    branch: Option<&str>,
) -> Result<()> {
    let root = super::ledger::store_root(start)?;
    let Some(config) = publisher(&root)? else {
        return Ok(());
    };
    ensure!(
        !super::ledger::contained(),
        "contained callers cannot publish authorship"
    );
    ensure!(
        head_sha.len() == 40 && head_sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "authorship needs the exact full head"
    );
    let store = store(&root).context("authorship requires distributed store")?;
    let (_, error) = authority::load_ledger_checked(&root);
    ensure!(error.is_none(), "cannot read local author identities");
    let snapshot = Snapshot::at_head(&root)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .context("missing canonical author snapshot")?;
    // A change keeps the spec set it was opened for: a rework head's commit
    // naming more (or no) IDs neither grows nor empties it.
    let (_, bound) = read_authorships(&root, change)?;
    let keys: Vec<SpecKey> = if bound.is_empty() {
        let set = snapshot.resolve(labels);
        ensure!(
            !set.members.is_empty() && set.unresolved.is_empty() && set.ambiguous.is_empty(),
            "unresolved author specs"
        );
        set.members.iter().map(|m| m.key).collect()
    } else {
        bound
    };
    let labels = spec_labels(&snapshot, &keys)?;
    let authors = authority::author_sessions_at(&root, &labels, None);
    ensure!(
        !authors.is_empty(),
        "cannot publish missing author identities"
    );
    let authors: Vec<Vec<String>> = authors
        .iter()
        .map(|a| qualify(&config.node, &a.tokens))
        .collect();
    ensure!(
        authors
            .iter()
            .all(|a| a.iter().any(|t| t.starts_with("session:"))),
        "authors need stable session identities"
    );
    let evidence = Authorship {
        version: 1,
        node: config.node.clone(),
        publication: Uuid::new_v4(),
        repository: repository(start)?,
        change: change.as_key(),
        head: head_sha.to_ascii_lowercase(),
        branch: branch.map(str::to_string),
        specs: keys,
        authors,
    };
    let signed = sign(&config, &evidence)?;
    verify_signature(&config.node, &signed, &roster(&store, &head(&store)?)?)?;
    persist(
        &store,
        &[(
            evidence_path(
                &evidence.change,
                AUTHORSHIPS,
                &evidence.node,
                evidence.publication,
            ),
            serde_json::to_vec(&signed)?,
        )],
        true,
    )
}

/// The host hook for every AIDA open of or push to a change: bind author
/// sessions to the pushed branch tip. Non-fatal: the change already exists
/// on the forge, so a failure is reported with its retry, never a failed
/// open (which would invite a duplicate PR).
pub(crate) fn publish_change_authorship(
    start: &Path,
    change: ChangeKey,
    branch: &str,
    specs: &[String],
) {
    let configured = super::ledger::store_root(start)
        .ok()
        .and_then(|root| publisher(&root).ok().flatten())
        .is_some();
    if !configured || change.number == 0 {
        return;
    }
    let result = branch_tip(start, branch)
        .with_context(|| format!("cannot resolve the pushed tip of {branch}"))
        .and_then(|tip| publish_authorship(start, change, specs, &tip, Some(branch)));
    if let Err(e) = result {
        eprintln!(
            "warning: {} is open, but its author publication is pending: {e:#}\n  retry from \
             a checkout of {branch}: aida review authorship --pr {} --specs {}",
            change.as_key(),
            change.number,
            specs.join(",")
        );
    }
}

/// The CLI form: bind the current checkout's HEAD and branch.
pub(crate) fn publish_checkout_authorship(start: &Path, pr: u64, specs: &[String]) -> Result<()> {
    let sha = head(start)?;
    let branch = text(git(start, &["rev-parse", "--abbrev-ref", "HEAD"])?)?;
    let branch = (branch != "HEAD").then_some(branch);
    publish_authorship(start, ChangeKey::pr(pr), specs, &sha, branch.as_deref())
}

/// Every label of each spec in `keys`, from the canonical snapshot.
fn spec_labels(snapshot: &Snapshot, keys: &[SpecKey]) -> Result<Vec<String>> {
    let mut labels = Vec::new();
    for key in keys {
        labels.extend(
            snapshot
                .spec(*key)
                .context("bound review spec disappeared")?
                .labels
                .iter()
                .cloned(),
        );
    }
    Ok(labels)
}

/// Signed author sessions for `change` and its bound spec set: the authors
/// are unioned over every head published for it (authors only accumulate, so
/// a rework or update-branch head keeps the earlier heads' exclusions); the
/// spec set is what the change was opened for. Only enrolled-node signed
/// evidence counts.
fn author_evidence(root: &Path, change: ChangeKey) -> Result<(Vec<Vec<String>>, Vec<SpecKey>)> {
    let (authors, covered) = read_authorships(root, change)?;
    ensure!(
        !covered.is_empty()
            && !authors.is_empty()
            && authors
                .iter()
                .all(|a| a.iter().any(|t| t.starts_with("session:"))),
        "missing portable author identity evidence; publish authorship for the change first"
    );
    Ok((authors, covered))
}

/// The raw signed authorship of `change` (empty when none is published).
fn read_authorships(root: &Path, change: ChangeKey) -> Result<(Vec<Vec<String>>, Vec<SpecKey>)> {
    let store = store(root).context("missing author store")?;
    let tip = head(&store)?;
    let roster = roster(&store, &tip)?;
    let key = change.as_key();
    let names = list(&store, &tip, &evidence_dir(&key, AUTHORSHIPS))?;
    let repository = repository(root)?;
    let mut authors = Vec::new();
    let mut covered = Vec::new();
    let started = std::time::Instant::now();
    let mut bytes_read = 0;
    for name in names {
        ensure!(
            started.elapsed() < Duration::from_secs(20),
            "authorship read deadline exhausted"
        );
        let bytes = blob(&store, &tip, &name)?.context("missing authorship")?;
        bytes_read += bytes.len();
        ensure!(bytes_read <= 8 * CAP, "authorship byte budget exhausted");
        let signed: Signed = serde_json::from_slice(&bytes)?;
        let value: Authorship = serde_json::from_str(&signed.payload)?;
        ensure!(
            value.version == 1
                && value.change == key
                && name == evidence_path(&key, AUTHORSHIPS, &value.node, value.publication),
            "invalid authorship identity/version"
        );
        ensure!(
            serde_json::to_string(&value)? == signed.payload,
            "noncanonical authorship"
        );
        if value.repository != repository {
            continue;
        }
        verify_signature(&value.node, &signed, &roster)?;
        covered.extend(value.specs);
        authors.extend(value.authors);
    }
    // Only signed change evidence establishes the author set. Unrelated
    // local leases cannot fill missing remote author evidence.
    ensure!(
        head(&store)? == tip,
        "store moved while reading author evidence"
    );
    authors.sort();
    authors.dedup();
    covered.sort();
    covered.dedup();
    Ok((authors, covered))
}

pub(crate) fn publish_local(pr: u64, labels: &[String], findings: &[String]) -> Result<()> {
    let root = super::ledger::store_root(&std::env::current_dir()?)?;
    ensure!(
        root.join(CONFIG).is_file(),
        "configure this host publisher first"
    );
    let change = ChangeKey::pr(pr);
    let mut record = super::ledger::project::newest_verdict(&root, &change.as_key())
        .context("no local review verdict for change")?;
    record.insert(
        "blocking_finding_origin_ids".into(),
        serde_json::to_value(findings)?,
    );
    publish(&root, change, &record, labels)
}

#[cfg(test)]
#[path = "tests/story_2492_publication_tests.rs"]
mod tests;

// Keep entry-point coverage mounted on the protected gate floor.
// trace:STORY-2492 | ai:codex
#[cfg(test)]
#[path = "tests/story_2492_entry_tests.rs"]
mod entry_tests;

/// The checked canonical gate report shares the same synced selection. Local
/// hold events retain their lifecycle; local verdict/carry-over evidence cannot
/// grant credit beyond the exact signed publication head.
pub(crate) fn overlay_view(
    root: &Path,
    change: ChangeKey,
    set: &super::canon::CanonicalSet,
    view: &mut super::effective::LedgerView,
) -> Result<()> {
    let Some((receipt, _)) = select(root, change, &[], None)? else {
        return Ok(());
    };
    let text = body(&receipt)?;
    let verdict = rv::parse_recorded_verdict(&text).context("invalid publication projection")?;
    if verdict.kind.approves() {
        verified_identity(root, &verdict)?;
    }
    let obj: rv::JsonObj = serde_json::from_str(&text)?;
    view.change_events.retain(|event| {
        !matches!(
            event.body,
            ledger::EventBody::Verdict { .. }
                | ledger::EventBody::LegacyImported { .. }
                | ledger::EventBody::CarryOverProof { .. }
                | ledger::EventBody::CarryOverRefused { .. }
        )
    });
    view.unbound.clear();
    for member in &set.members {
        if !receipt.specs.contains(&member.key) {
            continue;
        }
        let mut event = ledger::GateEvent::new(
            change,
            ledger::EventBody::Verdict {
                record: obj.clone(),
            },
        );
        event.head = Some(receipt.head.clone());
        event.at = receipt.recorded_at.clone();
        event.spec = Some(member.native.clone());
        event.spec_key = Some(member.key);
        event.actor = verdict.attestation.clone();
        view.change_events.push(event);
    }
    Ok(())
}

// trace:STORY-2698 | ai:codex
#[cfg(test)]
mod story_2698_tests {
    use super::*;

    #[test]
    fn story_2698_signed_payload_keeps_disposition_and_old_encoding() {
        let legacy = serde_json::json!({
            "version":1,"node":"test-node","publication":Uuid::nil(),"supersedes":[],
            "repository":"repo","change":"PR-7","head":"1111111111111111111111111111111111111111",
            "specs":[],"blocking_finding_origin_ids":[],"authors":[],"reviewer_actor":"reviewer",
            "reviewer_identity":[],"decision":"approved","recorded_at":"2026-11-09T00:00:00Z",
            "branch":null,"summary":null,"findings":["small bug"]
        });
        let mut receipt: Receipt = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(serde_json::to_value(&receipt).unwrap(), legacy);
        let mut clean = legacy.clone();
        clean["findings"] = serde_json::json!([]);
        let clean_receipt: Receipt = serde_json::from_value(clean.clone()).unwrap();
        let encoded = serde_json::to_string(&clean_receipt).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&encoded).unwrap(),
            clean
        );
        let restored: Receipt = serde_json::from_str(&encoded).unwrap();
        let projected = body(&restored).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&projected)
            .unwrap()
            .get("finding_dispositions")
            .is_none());
        assert_eq!(
            rv::parse_recorded_verdict(&projected).unwrap().kind,
            rv::VerdictKind::Approved
        );
        assert_eq!(
            rv::parse_recorded_verdict(&body(&receipt).unwrap())
                .unwrap()
                .kind,
            rv::VerdictKind::RequestChanges
        );
        receipt.finding_dispositions = Some(vec![
            serde_json::json!({"severity":"minor","disposition":"deferred","rationale":"safe follow-up"}),
        ]);
        let encoded = serde_json::to_string(&receipt).unwrap();
        let restored: Receipt = serde_json::from_str(&encoded).unwrap();
        assert_eq!(restored, receipt);
        assert!(rv::parse_recorded_verdict(&body(&restored).unwrap())
            .unwrap()
            .kind
            .approves());
    }
}
