//! Trusted-node review publications. Only the host recorder/collector issues
//! these after validating local authority. No grant or human receipt travels.
//! Trust comes from the CURRENT committed roster, never the publication itself.
// trace:STORY-2492 | ai:codex
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

const ROSTER: &str = "reviews/v1/nodes.json";
const AUTHORSHIPS: &str = "reviews/v1/authorship";
const PUBLICATIONS: &str = "reviews/v1/publications";
const NAMESPACE: &str = "aida-review-receipt";
const CONFIG: &str = ".aida/review-publisher.json";
const CAP: usize = 1024 * 1024;
const MAX_PUBLICATIONS: usize = 256;

#[derive(Debug, clap::Subcommand)]
pub enum NodeCommand {
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
}
#[derive(Serialize, Deserialize)]
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

/// Preserve concurrent publications. A rejected exact push rebases the immutable
/// commit on the winner under the store writer lock; no snapshot rollback/reset.
fn persist(store: &Path, rel: &str, bytes: &[u8], immutable: bool) -> Result<()> {
    persist_checked(store, rel, bytes, immutable, None)
}
fn persist_checked(
    store: &Path,
    rel: &str,
    bytes: &[u8],
    immutable: bool,
    expected_head: Option<&str>,
) -> Result<()> {
    use aida_core::{db::with_store_write_lock, git_ops};
    // This new store writer has no enabled-WORK prepared transport adapter.
    aida_core::transport_policy::refuse_unsupported(store)?;
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
        ensure!(
            git(store, &["diff", "--name-only", "--", rel])?.is_empty(),
            "review evidence has uncommitted edits"
        );
        let path = store.join(rel);
        if immutable && path.exists() {
            ensure!(
                std::fs::read(&path)? == bytes,
                "immutable review publication collision"
            );
        } else {
            let parent = path.parent().context("review publication parent")?;
            std::fs::create_dir_all(parent)?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            use std::io::Write;
            file.write_all(bytes)?;
            file.as_file().sync_all()?;
            file.persist(&path)?;
        }
        git_ops::add(store, &[rel])?;
        git_ops::commit(store, "review: publish trusted node evidence")?;
        Ok((git_ops::head_sha(store)?, git_ops::current_branch(store)?))
    })?;
    // Network publication is outside the store writer transaction. No reset or
    // rollback can remove another local writer's intervening commit.
    if !git_ops::has_remote(store, "origin") {
        return Ok(());
    }
    for _ in 0..3 {
        if git_ops::push_exact(store, "origin", &tip, &branch)? {
            return Ok(());
        }
        ensure!(
            immutable,
            "roster push raced another operator; sync and reapply the enrollment/revocation"
        );
        // Distinct immutable paths rebase cleanly. Never structurally auto-merge
        // a conflicting pending operator roster edit: that could undo revocation.
        git_ops::pull_rebase(store, "origin", &branch)?;
        tip = with_store_write_lock(store, || -> Result<String> {
            git_ops::ensure_store_write_safe(store)?;
            let tip = head(store)?;
            ensure!(
                blob(store, &tip, rel)?.as_deref() == Some(bytes),
                "review publication changed during synchronization"
            );
            Ok(tip)
        })?;
    }
    bail!("review publication remains local pending sync: push rejected")
}

pub(crate) fn nodes(command: &NodeCommand) -> Result<()> {
    authority_operator()?;
    let root = std::env::current_dir()?;
    let root = super::ledger::store_root(&root)?;
    let store = store(&root).context("review nodes require a distributed store")?;
    let (tip, mut roster) = aida_core::db::with_store_write_lock(&store, || -> Result<_> {
        let tip = head(&store)?;
        Ok((tip.clone(), roster(&store, &tip)?))
    })?;
    match command {
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
            ensure!(
                !roster.nodes.contains_key(node),
                "node already enrolled; revoke before changing its key"
            );
            roster.nodes.insert(node.clone(), key);
            persist_checked(
                &store,
                ROSTER,
                &serde_json::to_vec(&roster)?,
                false,
                Some(&tip),
            )
        }
        NodeCommand::Revoke { node } => {
            ensure!(roster.nodes.remove(node).is_some(), "node is not enrolled");
            persist_checked(
                &store,
                ROSTER,
                &serde_json::to_vec(&roster)?,
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
        ChangeKey::parse(&receipt.change).is_some() && !receipt.specs.is_empty(),
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
            "approved" | "request-changes" | "rejected"
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

/// Called only on the host writer path. A configuration opts that node into
/// publication. Revalidate the ordinary local authority before issuing a receipt.
pub(crate) fn publish(
    root: &Path,
    change: ChangeKey,
    record: &rv::JsonObj,
    labels: &[String],
) -> Result<()> {
    let config = match std::fs::read(root.join(CONFIG)) {
        Ok(bytes) => serde_json::from_slice::<Publisher>(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
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
    if verdict.kind.approves() {
        ensure!(
            super::review_approval::independent_approval_authorized(
                root,
                labels,
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
    let snapshot = Snapshot::at_head(root)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .context("missing canonical snapshot")?;
    let specs = snapshot.resolve(labels);
    ensure!(
        !specs.members.is_empty() && specs.unresolved.is_empty() && specs.ambiguous.is_empty(),
        "unresolved publication specs"
    );
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
    ensure!(
        verdict.kind.approves() || verdict.findings.is_empty() || !origins.is_empty(),
        "publish refusal findings by blocking_finding_origin_ids before syncing"
    );
    origins.sort();
    origins.dedup();
    let receipt = Receipt {
        version: 1,
        node: config.node.clone(),
        publication: Uuid::new_v4(),
        supersedes: publications(root)?
            .iter()
            .filter(|(r, _)| r.node == config.node && r.change == change.as_key())
            .map(|(r, _)| r.publication)
            .collect(),
        repository: repository(root)?,
        change: change.as_key(),
        head: verdict
            .reviewed_sha
            .clone()
            .context("missing exact review head")?,
        specs: specs.members.iter().map(|s| s.key).collect(),
        blocking_finding_origin_ids: origins,
        authors: author_evidence(
            root,
            change,
            verdict.reviewed_sha.as_deref().unwrap_or(""),
            &specs.members.iter().map(|s| s.key).collect::<Vec<_>>(),
        )?,
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
    };
    ensure!(
        receipt
            .authors
            .iter()
            .all(|tokens| !authority::identities_overlap(tokens, &receipt.reviewer_identity)),
        "reviewer is an author across nodes"
    );
    let signed = sign(&config, &receipt)?;
    verify(&receipt, &signed, &roster(&store, &head(&store)?)?)?;
    persist(
        &store,
        &format!(
            "{PUBLICATIONS}/{}/{}.json",
            receipt.node, receipt.publication
        ),
        &serde_json::to_vec(&signed)?,
        true,
    )
}

fn publications(root: &Path) -> Result<Vec<(Receipt, PathBuf)>> {
    let Some(store) = store(root) else {
        return Ok(Vec::new());
    };
    // A legacy/local-only store directory can be discovered without being a
    // Git repository. It has no publication authority. Any attempted review
    // namespace or explicit publisher there still goes through strict refusal.
    let absent = |path: &Path| matches!(std::fs::symlink_metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound);
    if absent(&store.join(".git"))
        && absent(&store.join("HEAD"))
        && absent(&store.join("reviews"))
        && absent(&root.join(CONFIG))
    {
        return Ok(Vec::new());
    }
    let tip = head(&store)?;
    let names = git(
        &store,
        &["ls-tree", "-r", "--name-only", &tip, "--", PUBLICATIONS],
    )?;
    let names = String::from_utf8(names)?;
    ensure!(
        names.lines().count() <= MAX_PUBLICATIONS,
        "review publication read budget exhausted"
    );
    let repository = if names.is_empty() {
        String::new()
    } else {
        repository(root)?
    };
    let mut out = Vec::new();
    let mut total = 0;
    let started = std::time::Instant::now();
    for name in names.lines() {
        ensure!(
            started.elapsed() < Duration::from_secs(20),
            "publication read deadline exhausted"
        );
        let bytes = blob(&store, &tip, name)?.context("publication vanished")?;
        total += bytes.len();
        ensure!(
            total <= 8 * CAP,
            "review publication aggregate budget exhausted"
        );
        let signed: Signed = serde_json::from_slice(&bytes)?;
        let receipt: Receipt = serde_json::from_str(&signed.payload)?;
        ensure!(
            name == format!(
                "{PUBLICATIONS}/{}/{}.json",
                receipt.node, receipt.publication
            ),
            "publication identity/path mismatch"
        );
        if receipt.repository != repository {
            continue;
        }
        out.push((receipt, store.join(name)));
    }
    ensure!(
        head(&store)? == tip,
        "review roster/store moved during publication validation"
    );
    Ok(out)
}
fn reference(r: &Receipt) -> String {
    format!("publication:{}:{}", r.node, r.publication)
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
    Ok(serde_json::to_string(&serde_json::json!({
        "verdict": r.decision, "reviewed_sha": r.head, "reviewed_branch": r.branch,
        "recorded_at": r.recorded_at, "recorded_by": r.reviewer_actor,
        "summary": r.summary, "findings": r.findings,
        "blocking_finding_origin_ids": r.blocking_finding_origin_ids,
        "recorder_attestation": att,
    }))?)
}

/// One current answer per node; concurrent independent refusals cannot be erased
/// by an approval on another node. UUIDs identify publications, not wall-clock
/// or local ledger sequence. Supersession is signed and scoped to one publisher.
fn current_for(
    root: &Path,
    ids: &[&str],
    expected_head: Option<&str>,
) -> Result<Option<rv::CurrentVerdict>> {
    let all = publications(root)?;
    if all.is_empty() {
        return Ok(None);
    }
    let snapshot = Snapshot::at_head(root)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .context("missing canonical review snapshot")?;
    let keys = snapshot
        .resolve(ids)
        .members
        .into_iter()
        .map(|s| s.key)
        .collect::<Vec<_>>();
    let mut relevant: Vec<_> = all
        .iter()
        .filter(|(r, _)| {
            ids.iter()
                .any(|id| ChangeKey::parse(id).is_some_and(|c| c.as_key() == r.change))
                || r.specs.iter().any(|k| keys.contains(k))
        })
        .collect();
    // A later publication can remove a spec from its coverage. Select the
    // whole change's causal history before checking the surviving coverage.
    let changes: Vec<_> = relevant.iter().map(|(r, _)| r.change.clone()).collect();
    relevant = all
        .iter()
        .filter(|(r, _)| changes.contains(&r.change))
        .collect();
    let store = store(root).context("missing review store")?;
    let tip = head(&store)?;
    let roster = roster(&store, &tip)?;
    let had_relevant = !relevant.is_empty();
    relevant.retain(|(r, _)| roster.nodes.contains_key(&r.node));
    ensure!(
        !had_relevant || !relevant.is_empty(),
        "all relevant review publishing nodes were revoked"
    );
    let verification_started = std::time::Instant::now();
    for (r, path) in &relevant {
        ensure!(
            verification_started.elapsed() < Duration::from_secs(20),
            "review verification deadline exhausted"
        );
        let rel = path
            .strip_prefix(&store)?
            .to_str()
            .context("publication path")?;
        let signed: Signed =
            serde_json::from_slice(&blob(&store, &tip, rel)?.context("missing publication")?)?;
        verify(r, &signed, &roster)?;
    }
    // Only verified relevant publications may supersede evidence. Unverified
    // bodies from another spec must never erase a refusal during alias selection.
    let mut pending: Vec<_> = relevant
        .iter()
        .map(|(r, _)| (r.node.clone(), r.publication))
        .collect();
    while !pending.is_empty() {
        let before = pending.len();
        let ready: Vec<_> = pending
            .iter()
            .filter(|(node, id)| {
                let r = relevant
                    .iter()
                    .find(|(r, _)| r.node == *node && r.publication == *id)
                    .unwrap()
                    .0
                    .clone();
                !r.supersedes
                    .iter()
                    .any(|old| pending.contains(&(node.clone(), *old)))
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
        .flat_map(|(r, _)| {
            r.supersedes
                .iter()
                .map(|id| (r.node.clone(), r.change.clone(), *id))
        })
        .collect();
    relevant
        .retain(|(r, _)| !superseded.contains(&(r.node.clone(), r.change.clone(), r.publication)));
    ensure!(
        !had_relevant || !relevant.is_empty(),
        "review publication selection lost all evidence"
    );
    if relevant.is_empty() {
        return Ok(None);
    }
    relevant.retain(|(r, _)| {
        ids.iter()
            .any(|id| ChangeKey::parse(id).is_some_and(|c| c.as_key() == r.change))
            || r.specs.iter().any(|k| keys.contains(k))
    });
    ensure!(
        !had_relevant || !relevant.is_empty(),
        "current publication no longer covers these specs"
    );
    if let Some(head) = expected_head {
        relevant.retain(|(r, _)| r.head.eq_ignore_ascii_case(head));
        ensure!(
            !had_relevant || !relevant.is_empty(),
            "no signed review publication matches the current change head"
        );
    }
    relevant.sort_by(|(a, _), (b, _)| (&a.node, a.publication).cmp(&(&b.node, b.publication)));
    let blocking = relevant.iter().find(|(r, _)| r.decision != "approved");
    let (receipt, path) = if let Some(blocking) = blocking {
        *blocking
    } else {
        let first = relevant[0];
        ensure!(
            relevant
                .iter()
                .all(|(r, _)| r.change == first.0.change && r.head == first.0.head),
            "concurrent review publications disagree on change/head"
        );
        first
    };
    if receipt.decision == "approved" {
        let requested = snapshot.resolve(ids);
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
    Ok(Some(rv::CurrentVerdict {
        body: body(receipt)?,
        source: rv::VerdictSource::File(path.clone()),
        written_at: None,
    }))
}

/// Synced evidence wins. Invalid evidence becomes an explicit non-approval;
/// Option-shaped legacy readers must never fall back to a local approval.
pub(crate) fn current(root: &Path, ids: &[&str]) -> Option<rv::CurrentVerdict> {
    resolve_current(root, ids, None)
}

pub(crate) fn current_at_head(root: &Path, ids: &[&str], head: &str) -> Option<rv::CurrentVerdict> {
    resolve_current(root, ids, Some(head))
}

fn resolve_current(root: &Path, ids: &[&str], head: Option<&str>) -> Option<rv::CurrentVerdict> {
    match current_for(root, ids, head) {
        Ok(value) => {
            if let Some(remote) = &value {
                for id in ids {
                    if let Some(local) = super::ledger::project::newest_verdict(root, id) {
                        let local = rv::parse_ledger_record(&local);
                        let remote = rv::parse_recorded_verdict(&remote.body);
                        if matches!((local, remote), (Some(a), Some(b)) if a.kind != b.kind || a.reviewed_sha != b.reviewed_sha || a.findings != b.findings)
                        {
                            eprintln!("review publication/local ledger disagreement for {id}; using synced evidence");
                        }
                    }
                }
            }
            value
        }
        Err(e) => {
            eprintln!("review publication refused: {e:#}");
            Some(rv::CurrentVerdict {
                body: serde_json::json!({"verdict":"request-changes", "summary":format!("review publication refused: {e:#}")}).to_string(),
                source: rv::VerdictSource::File(root.join(".aida-store").join(PUBLICATIONS)), written_at: None,
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
    let id = att
        .receipt_id
        .as_deref()
        .context("missing publication reference")?;
    let all = publications(root)?;
    let receipt = &all
        .iter()
        .find(|(r, _)| reference(r) == id)
        .context("publication not found")?
        .0;
    let store = store(root).context("missing publication store")?;
    let tip = head(&store)?;
    let rel = format!(
        "{PUBLICATIONS}/{}/{}.json",
        receipt.node, receipt.publication
    );
    let signed: Signed =
        serde_json::from_slice(&blob(&store, &tip, &rel)?.context("missing signed publication")?)?;
    verify(receipt, &signed, &roster(&store, &tip)?)?;
    let expected =
        rv::parse_recorded_verdict(&body(receipt)?).context("invalid publication projection")?;
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
    ensure!(
        !receipt.authors.is_empty()
            && receipt
                .authors
                .iter()
                .all(|tokens| tokens.iter().any(|t| t.starts_with("session:"))),
        "missing portable author identity evidence"
    );
    for tokens in &receipt.authors {
        ensure!(
            !authority::identities_overlap(tokens, &receipt.reviewer_identity),
            "reviewer is an author across nodes"
        );
    }
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
    let selected = current_for(root, &[&receipt.change], Some(&receipt.head))?
        .context("publication is no longer current")?;
    ensure!(
        selected.body == body(receipt)?,
        "publication was superseded or opposed by a refusal"
    );
    Ok(receipt.reviewer_identity.clone())
}

/// Portable author identity asserted by the trusted host opening a change.
/// Contains only session identities, not grant/receipt/process authority.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Authorship {
    version: u32,
    node: String,
    publication: Uuid,
    repository: String,
    change: String,
    head: String,
    specs: Vec<SpecKey>,
    authors: Vec<Vec<String>>,
}

pub(crate) fn publish_authorship(start: &Path, change: ChangeKey, labels: &[String]) -> Result<()> {
    let root = super::ledger::store_root(start)?;
    let config = match std::fs::read(root.join(CONFIG)) {
        Ok(bytes) => serde_json::from_slice::<Publisher>(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    ensure!(
        !super::ledger::contained(),
        "contained callers cannot publish authorship"
    );
    let store = store(&root).context("authorship requires distributed store")?;
    let (_, error) = authority::load_ledger_checked(&root);
    ensure!(error.is_none(), "cannot read local author identities");
    let snapshot = Snapshot::at_head(&root)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .context("missing canonical author snapshot")?;
    let set = snapshot.resolve(labels);
    ensure!(
        !set.members.is_empty() && set.unresolved.is_empty() && set.ambiguous.is_empty(),
        "unresolved author specs"
    );
    let labels: Vec<String> = set.members.iter().flat_map(|m| m.labels.clone()).collect();
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
        head: head(start)?,
        specs: set.members.iter().map(|s| s.key).collect(),
        authors,
    };
    let signed = sign(&config, &evidence)?;
    verify_signature(&config.node, &signed, &roster(&store, &head(&store)?)?)?;
    persist(
        &store,
        &format!(
            "{AUTHORSHIPS}/{}/{}.json",
            evidence.node, evidence.publication
        ),
        &serde_json::to_vec(&signed)?,
        true,
    )
}
fn author_evidence(
    root: &Path,
    change: ChangeKey,
    sha: &str,
    required_specs: &[SpecKey],
) -> Result<Vec<Vec<String>>> {
    let store = store(root).context("missing author store")?;
    let tip = head(&store)?;
    let roster = roster(&store, &tip)?;
    let names = text(git(
        &store,
        &["ls-tree", "-r", "--name-only", &tip, "--", AUTHORSHIPS],
    )?)?;
    ensure!(
        names.lines().count() <= MAX_PUBLICATIONS,
        "authorship read budget exhausted"
    );
    let repository = repository(root)?;
    let mut authors = Vec::new();
    let mut covered = Vec::new();
    let started = std::time::Instant::now();
    let mut bytes_read = 0;
    for name in names.lines() {
        ensure!(
            started.elapsed() < Duration::from_secs(20),
            "authorship read deadline exhausted"
        );
        let bytes = blob(&store, &tip, name)?.context("missing authorship")?;
        bytes_read += bytes.len();
        ensure!(bytes_read <= 8 * CAP, "authorship byte budget exhausted");
        let signed: Signed = serde_json::from_slice(&bytes)?;
        let value: Authorship = serde_json::from_str(&signed.payload)?;
        ensure!(
            value.version == 1
                && name == format!("{AUTHORSHIPS}/{}/{}.json", value.node, value.publication),
            "invalid authorship identity/version"
        );
        ensure!(
            serde_json::to_string(&value)? == signed.payload,
            "noncanonical authorship"
        );
        if value.repository != repository || value.change != change.as_key() || value.head != sha {
            continue;
        }
        verify_signature(&value.node, &signed, &roster)?;
        covered.extend(value.specs);
        authors.extend(value.authors);
    }
    // Only signed exact-change/head evidence establishes the author set.
    // Unrelated local leases cannot fill missing remote author evidence.
    ensure!(
        required_specs.iter().all(|key| covered.contains(key)),
        "missing portable author spec coverage"
    );
    ensure!(
        covered.iter().all(|key| required_specs.contains(key)),
        "review publication omits authored PR specs"
    );
    ensure!(
        !authors.is_empty()
            && authors
                .iter()
                .all(|a| a.iter().any(|t| t.starts_with("session:"))),
        "missing portable author identity evidence; publish authorship at the current head first"
    );
    ensure!(
        head(&store)? == tip,
        "store moved while reading author evidence"
    );
    authors.sort();
    authors.dedup();
    Ok(authors)
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

/// The checked canonical gate report shares the same synced selection. Local
/// hold events retain their lifecycle; local verdict/carry-over evidence cannot
/// grant credit beyond the exact signed publication head.
pub(crate) fn overlay_view(
    root: &Path,
    change: ChangeKey,
    set: &super::canon::CanonicalSet,
    view: &mut super::effective::LedgerView,
) -> Result<()> {
    let Some(remote) = current_for(root, &[&change.as_key()], None)? else {
        return Ok(());
    };
    let verdict =
        rv::parse_recorded_verdict(&remote.body).context("invalid publication projection")?;
    if verdict.kind.approves() {
        verified_identity(root, &verdict)?;
    }
    let obj: rv::JsonObj = serde_json::from_str(&remote.body)?;
    let all = publications(root)?;
    let att = verdict
        .attestation
        .as_ref()
        .context("missing review publication identity")?;
    let receipt = &all
        .iter()
        .find(|(r, _)| att.receipt_id.as_deref() == Some(reference(r).as_str()))
        .context("missing review publication")?
        .0;
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
