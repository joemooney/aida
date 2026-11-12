//! Invocation-only mechanical train coverage. Stored evidence is never an authorization.
// trace:BUG-b6b1278890774ef993d2ebc327862470-179 | ai:codex

use super::*;
use crate::forge::{ChangeRef, Forge, MergeMethod, MergeOptions};
use crate::gate::{approval, ledger, merge_hold_core as corpus, review_verdict as rv};
use serde_json::{json, Value};

const PATCH_VERSION: u32 = 1; // binary/full-index diff + git patch-id --verbatim

type Bodies = Vec<(String, Value)>;

/// Private fields, no Deserialize/Clone or public constructor. The borrowed
/// refresh owns the normal live ports and lease for this one merge invocation.
pub(crate) struct VerifiedTrainMerge<'a> {
    root: PathBuf,
    change: ChangeRef,
    tip: String,
    evidence: Value,
    mapped: Bodies,
    refresh: Box<dyn FnMut() -> Result<(Value, Bodies)> + 'a>,
    /// BUG-2433: a final gate passed, so a forge merge call was issued.
    reached_forge: bool,
}

impl VerifiedTrainMerge<'_> {
    /// Called by the shared production forge gate before EACH transport attempt.
    pub(crate) fn enforce(
        &mut self,
        forge: &dyn Forge,
        root: &Path,
        change: &ChangeRef,
        opts: &MergeOptions,
    ) -> Result<()> {
        anyhow::ensure!(
            root.canonicalize()? == self.root
                && change == &self.change
                && opts.match_head.as_deref() == Some(self.tip.as_str())
                && opts.method == MergeMethod::Rebase,
            "train merge identity or pin changed"
        );
        let (evidence, mapped) = (self.refresh)()?;
        anyhow::ensure!(
            evidence == self.evidence && mapped == self.mapped,
            "train proof or original source review evidence changed; rebuild and retest"
        );
        // A persisted audit is required, but it cannot create this value or
        // substitute for the live refresh above.
        let store = ledger::store_root(root)?;
        validate_ledger(&store, &format!("PR-{}", change.id))?;
        anyhow::ensure!(ledger::project::events(&store, ledger::ChangeKey::pr(change.id))
            .iter().any(|e| matches!(&e.body, ledger::EventBody::TrainCoverage { proof } if proof == &self.evidence)),
            "train coverage audit missing or changed");
        let mut hints: Vec<String> = self.evidence["specs"]
            .as_array()
            .context("train specs missing")?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        // Never narrower than the standalone gate: union the source aliases
        // with the ordinary hints for the train change itself (its branch,
        // title and commit-trailer ids).
        // trace:BUG-b6b1278890774ef993d2ebc327862470-179 | ai:claude
        hints.extend(crate::forge::corpus_gate_spec_hints(root, change));
        hints
            .iter_mut()
            .for_each(|id| *id = id.to_ascii_uppercase());
        hints.sort();
        hints.dedup();
        let bodies = read_bodies(root, change.id, &hints, &self.tip)?;
        let live_bodies: Vec<String> = bodies
            .iter()
            .map(|(_, record)| serde_json::to_string(record))
            .collect::<std::result::Result<_, _>>()?;
        // Inspect the complete corpus BEFORE mapping source artifacts away:
        // an actual at-tip blocker/conflict in a record's round history still
        // holds the train, even when that record also carries source evidence.
        let at_tip = rv::reconcile_artifacts_for_sha(
            live_bodies
                .iter()
                .filter(|body| rv::parse_recorded_verdict(body).is_none_or(|v| !v.is_closed()))
                .map(String::as_str),
            &self.tip,
        );
        if !match at_tip {
            Ok(None) => true,
            Ok(Some(kind)) => kind.approves(),
            Err(_) => false,
        } {
            let all: Vec<_> = bodies
                .iter()
                .zip(&live_bodies)
                .map(|((key, _), body)| (key.clone(), body.clone()))
                .collect();
            corpus::enforce_corpus_decision(
                root,
                change.id,
                corpus::corpus_merge_gate(change.id, &all, Some(self.tip.as_str())),
            )?;
        }
        let remaining: Vec<(String, String)> = bodies
            .into_iter()
            .filter(|(key, record)| {
                !self.mapped.iter().any(|(mapped_key, original)| {
                    mapped_key.eq_ignore_ascii_case(key) && original == record
                })
            })
            .map(|(key, record)| Ok((key, serde_json::to_string(&record)?)))
            .collect::<Result<_>>()?;
        // Original source artifacts cover only their proved cars. Everything
        // else still passes the ordinary exact-tip corpus decision (including
        // actual train-head blockers); no virtual at-tip verdict is created.
        corpus::enforce_corpus_decision(
            root,
            change.id,
            corpus::corpus_merge_gate(change.id, &remaining, Some(self.tip.as_str())),
        )?;
        let meta = forge.change_metadata(change.id, &mut crate::network_retry::NoopSink)?;
        anyhow::ensure!(
            meta.head_sha == self.tip,
            "train head moved during final gate"
        );
        // trace:BUG-2433 | ai:claude
        self.reached_forge = true;
        Ok(())
    }
}

/// The sole constructor and consumer. Called from land with its actual guard;
/// normal member gates are repeated here and before each retry, under that guard.
pub(super) fn merge(
    live: &mut LiveTrainForge<'_>,
    change: &ChangeRef,
    train: &Built,
    lease: &dyn std::any::Any,
) -> Result<()> {
    let lease = lease
        .downcast_ref::<crate::merge_lock::MergeLease>()
        .context("verified train merge requires the normal live merge lease")?;
    let forge = live.forge;
    let opts = MergeOptions {
        method: MergeMethod::Rebase,
        squash_subject: None,
        squash_body: None,
        delete_branch: true,
        match_head: Some(train.tip.clone()),
    };
    let mut context = verified(live, change, train, lease)?;
    let merged = forge.merge_train_change(
        change,
        &opts,
        &mut crate::network_retry::StderrSink,
        &mut context,
    );
    // BUG-2433: only an error from (or after) an issued merge call can hide a
    // merge that took effect; a refusal before any call stays untagged, so
    // it surfaces at once instead of waiting through post-error re-reads.
    // trace:BUG-2433 | ai:claude
    match merged {
        Ok(_) => Ok(()),
        Err(e) if context.reached_forge => Err(e.context(ForgeMergeCallFailed)),
        Err(e) => Err(e),
    }
}

fn verified<'a>(
    live: &'a mut LiveTrainForge<'_>,
    change: &'a ChangeRef,
    train: &'a Built,
    lease: &'a crate::merge_lock::MergeLease,
) -> Result<VerifiedTrainMerge<'a>> {
    let (evidence, mapped) = collect(live, change, train, lease)?;
    let root = live.project_root.canonicalize()?;
    let store = ledger::store_root(&root)?;
    let mut event = ledger::GateEvent::new(
        ledger::ChangeKey::pr(change.id),
        ledger::EventBody::TrainCoverage {
            proof: evidence.clone(),
        },
    );
    event.head = Some(train.tip.clone());
    event.base_ref = Some(live.base.clone());
    event.base_tip = Some(train.base.clone());
    ledger::record_event(&store, event).context("persisting train coverage audit")?;
    Ok(VerifiedTrainMerge {
        root,
        change: change.clone(),
        tip: train.tip.clone(),
        evidence,
        mapped,
        refresh: Box::new(|| collect(live, change, train, lease)),
        reached_forge: false,
    })
}

fn collect(
    live: &mut LiveTrainForge<'_>,
    change: &ChangeRef,
    train: &Built,
    lease: &crate::merge_lock::MergeLease,
) -> Result<(Value, Bodies)> {
    live.dispatch_check()?;
    crate::standalone_implementer::refuse_under_marker("pr train")?;
    anyhow::ensure!(
        lease.covers(&live.main_worktree, &live.base, change.id),
        "train merge lease lost"
    );
    anyhow::ensure!(
        live.forge.kind() == crate::forge::ForgeKind::GitHub,
        "unsupported train forge"
    );
    let origin = git(&live.main_worktree, &["remote", "get-url", "origin"])?;
    let repository = live.main_worktree.canonicalize()?;
    let protected = crate::gate::review_carryover::protected_base(&live.main_worktree)
        .map_err(|e| anyhow::anyhow!(e.reason()))?;
    anyhow::ensure!(
        protected == live.base && change.base == live.base,
        "train protected base changed"
    );
    anyhow::ensure!(
        live.fetch_main()? == train.base,
        "train main moved; rebuild and retest"
    );
    let cars = verify_structure(&live.main_worktree, train)?;
    let meta = live
        .forge
        .change_metadata(change.id, &mut crate::network_retry::NoopSink)?;
    anyhow::ensure!(
        meta.state == crate::forge::ChangeState::Open
            && !meta.is_draft
            && !meta.is_cross_repository
            && meta.base_ref == live.base
            && meta.head_sha == train.tip
            && meta.head_ref == change.branch,
        "train metadata or tested head changed"
    );
    anyhow::ensure!(
        crate::pr_cmd::probe_ship_drive_block(&live.main_worktree, change.id).is_none(),
        "a live drive owns the train"
    );
    anyhow::ensure!(
        !matches!(
            crate::gate::review_marker::merge_gate(&live.main_worktree, change.id, || Some(
                train.tip.clone()
            )),
            crate::gate::review_marker::MergeGate::UnderReview(_)
        ),
        "train is under review"
    );
    anyhow::ensure!(
        corpus::read_hold(&live.main_worktree, change.id).is_none(),
        "train has a merge hold"
    );
    let labels =
        crate::merge_hold::fetch_pinned_change(&live.project_root, live.forge.kind(), change.id)
            .map_err(anyhow::Error::msg)?;
    if let Some(labels) = labels {
        anyhow::ensure!(
            member_label_refusal(&labels, false).is_none(),
            "train labels hold the merge"
        );
    }
    // Read CI by exact commit, never by branch. Required-check discovery must
    // itself succeed; an API error is not an empty required-check policy.
    let required = crate::forge::github_required_checks(&live.project_root, change)?;
    // Bind the live CI read and every refreshed proof to its workflow content.
    // trace:STORY-2401 | ai:codex
    let ci_key =
        crate::ci_digest::CiEvidenceKey::read(&live.project_root, &train.tip, live.forge.kind())?;
    let names: Vec<&str> = required.iter().map(String::as_str).collect();
    // Hold release can re-run a required context even after the PR-level
    // wait passed. Every proof refresh must settle the exact commit too.
    // trace:BUG-2528 | ai:codex
    let started = std::time::Instant::now();
    wait_exact_tip_ci(
        &ci_key.head_sha,
        &names,
        180,
        || {
            live.dispatch_check()?;
            live.forge.commit_checks(&ci_key.head_sha)
        },
        || started.elapsed().as_secs(),
        || std::thread::sleep(std::time::Duration::from_secs(TRAIN_CI_POLL_SECS)),
    )?;
    let mut sources = Vec::new();
    let mut mapped = Vec::new();
    // trace:BUG-2438 | ai:claude
    let mut superseded = Vec::new();
    let mut all_specs = Vec::new();
    for car in &train.cars {
        if let Some(cover) = &car.member.release {
            anyhow::ensure!(
                crate::gate::mandate_ship::mandate_still_active(&cover.id),
                "train release mandate expired or was revoked"
            );
        }
        let fresh = live.gate(car.member.pr).map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            fresh.head == car.member.head
                && fresh.subject == car.member.subject
                && fresh.body == car.member.body,
            "train source head or ship metadata changed"
        );
        let source = live
            .forge
            .change_metadata(fresh.pr, &mut crate::network_retry::NoopSink)?;
        anyhow::ensure!(
            source.head_sha == fresh.head && member_metadata_refusal(&source, &live.base).is_none(),
            "source metadata moved during the train gate"
        );
        let metadata = json!({"state":format!("{:?}",source.state), "title":source.title,
            "author":source.author,"base":source.base_ref,"branch":source.head_ref,"head":source.head_sha,
            "draft":source.is_draft,"cross_repository":source.is_cross_repository,"repository":source.head_repo});
        let ids = resolved_member_spec_ids(
            &live.project_root,
            &source.head_ref,
            &source.title,
            &fresh.subject,
            &fresh.body,
        )?;
        let covered = crate::gate::trailer_coverage::covered_gate_ids(
            &live.project_root,
            &source.head_ref,
            Some(fresh.pr),
            ids.clone(),
        );
        // trace:TASK-3111 trace:TASK-2843 | ai:codex
        anyhow::ensure!(
            ids.is_empty() || !covered.is_empty(),
            "train source specs are uncovered by the branch/lease coverage filter"
        );
        let ids = aliases(&live.project_root, &covered)?;
        let mut policy_specs =
            crate::pr_cmd::ship_gate_spec_records_for_ids(&live.project_root, ids.clone());
        for (_, tags, _) in &mut policy_specs {
            tags.sort();
            tags.dedup();
        }

        // The exact-head publication answers first, as it does at the member
        // gate. Without one, alias-aware local inputs retain every conflict.
        // These reads do not change M1 binding or spec-only routing.
        let bodies = read_bodies(&live.project_root, fresh.pr, &ids, &fresh.head)?;
        anyhow::ensure!(!bodies.is_empty(), "train source approval evidence missing");
        corpus::enforce_corpus_gate_before_merge(
            &live.project_root,
            fresh.pr,
            &ids,
            Some(&live.base),
            || Some(fresh.head.clone()),
        )?;
        let mut verdicts = Vec::new();
        for (key, record) in &bodies {
            let body = serde_json::to_string(record)?;
            let verdict = rv::parse_recorded_verdict(&body).context("unparseable source review")?;
            verdicts.push((key, record, verdict));
        }
        // Head-scoped effective verdict, the same reading the corpus gate
        // above applies: once any of the PR's keys holds a verdict reviewed
        // at the current head, a verdict reviewed at an older head is
        // superseded (it is a past round, not the standing decision). With
        // no current-head verdict every body still counts, so an off-head
        // approval stands only as the patch-identity carry-over the corpus
        // gate proved. At the same head, append-order selection is shared
        // with the corpus gate above.
        // trace:BUG-2438 | ai:claude
        let reviewed_at_head = |v: &rv::RecordedVerdict| {
            v.reviewed_sha
                .as_deref()
                .is_some_and(|sha| rv::same_reviewed_sha(sha, &fresh.head))
        };
        let at_head = verdicts
            .iter()
            .any(|(_, _, v)| !v.is_closed() && reviewed_at_head(v));
        // trace:BUG-2474 trace:TASK-2964 | ai:codex
        let raw_bodies: Vec<_> = verdicts
            .iter()
            .filter(|(_, _, verdict)| !verdict.is_closed())
            .map(|(_, record, _)| serde_json::to_string(record))
            .collect::<std::result::Result<_, _>>()?;
        let effective =
            rv::effective_recordings_for_sha(raw_bodies.iter().map(String::as_str), &fresh.head)
                .map_err(anyhow::Error::msg)?;
        let mut originals = Vec::new();
        let mut superseded_here = Vec::new();
        for (key, record, verdict) in verdicts {
            if verdict.is_closed() {
                continue;
            }
            // Only a well-formed SHA (>= 7 hex) naming a different commit is
            // an older head; a malformed or too-short one keeps counting, so
            // the refusal it carries fails closed.
            let mut identity = record
                .as_object()
                .context("source review is not an object")?
                .clone();
            identity.remove("rounds");
            identity.remove("unarchived_rounds");
            let is_superseded = at_head
                && verdict.reviewed_sha.as_deref().is_some_and(|sha| {
                    let sha = sha.trim();
                    sha.len() >= 7
                        && sha.bytes().all(|c| c.is_ascii_hexdigit())
                        && (!rv::same_reviewed_sha(sha, &fresh.head)
                            || !effective.contains(&identity))
                });
            if is_superseded {
                // Covered by this car's current-head verdict, so the train's
                // own exact-tip corpus gate must not re-weigh it either.
                superseded_here.push(json!({"key": key, "record": record}));
                if !superseded.contains(&(key.clone(), record.clone())) {
                    superseded.push((key.clone(), record.clone()));
                }
                continue;
            }
            anyhow::ensure!(
                verdict.kind.approves(),
                "train source has a refusing review"
            );
            let policy = approval::ship_approval_policy(
                &live.main_worktree,
                &ids,
                Some(&source.head_ref),
                std::slice::from_ref(&verdict),
            );
            anyhow::ensure!(
                crate::gate::ship::approval_authority_refusal(
                    std::slice::from_ref(&verdict),
                    &policy
                )
                .is_none(),
                "train original review authority refused"
            );
            let reference = ledger::project::newest_verdict_event(&live.project_root, key);
            originals.push(json!({"key": key, "record": record, "ledger_reference": reference}));
            if !mapped.contains(&(key.clone(), record.clone())) {
                mapped.push((key.clone(), record.clone()));
            }
        }
        anyhow::ensure!(
            !originals.is_empty(),
            "train car has no original authorized approval"
        );
        sources.push(json!({"pr": fresh.pr, "head": fresh.head, "branch": source.head_ref, "specs": ids, "originals": originals, "superseded": superseded_here, "metadata":metadata, "policy_specs":policy_specs}));
        all_specs.extend(ids);
    }
    all_specs.sort();
    all_specs.dedup();
    // The train-key exemption is ONLY the selected M1 copy bound to one of
    // these covered source aliases. Content equality at an unrelated key or
    // with an unrelated spec is not provenance. Pin the exact event too.
    let train_key = format!("PR-{}", change.id);
    let bound = ledger::project::newest_verdict_event(&live.project_root, &train_key)
        .filter(|event| {
            event
                .spec
                .as_ref()
                .is_some_and(|spec| all_specs.iter().any(|id| id.eq_ignore_ascii_case(spec)))
        })
        .filter(|event| {
            event.verdict_record().is_some_and(|record| {
                mapped
                    .iter()
                    .any(|(_, original)| original == &Value::Object(record.clone()))
            })
        });
    if let Some(event) = &bound {
        // trace:TASK-2964 | ai:codex
        // The exact bound event also owns its derived append-history views.
        // The at-tip gate still checks all retained evidence before mapping.
        let store = ledger::store_root(&live.project_root)?;
        for body in rv::bodies_with_ledger_history(&store, event).map_err(anyhow::Error::msg)? {
            mapped.push((train_key.clone(), serde_json::from_str(&body)?));
        }
    }
    // Superseded bodies join the mapped set only after the train-key binding
    // above, so an older-head refusal can never be the M1 copy it binds.
    // trace:BUG-2438 | ai:claude
    for entry in superseded {
        if !mapped.contains(&entry) {
            mapped.push(entry);
        }
    }
    anyhow::ensure!(
        live.fetch_main()? == train.base,
        "main moved during train re-gate"
    );
    Ok((
        json!({"v": PATCH_VERSION, "repository": repository, "origin": origin,
        "change": change.id, "branch": change.branch, "base_ref": live.base,
        "base_tip": train.base, "tip": train.tip, "ci_key": ci_key, "cars": cars, "sources": sources,
        "specs": all_specs, "bound_source_event": bound}),
        mapped,
    ))
}

/// Only an in-flight context may defer the final decision. Never substitute
/// branch CI, an earlier green snapshot, or an unreadable observation.
// trace:BUG-2528 | ai:codex
fn wait_exact_tip_ci(
    tip: &str,
    required: &[&str],
    limit_secs: u64,
    mut snapshot: impl FnMut() -> Result<crate::forge::CommitChecks>,
    mut elapsed: impl FnMut() -> u64,
    mut sleep: impl FnMut(),
) -> Result<()> {
    loop {
        let checks = snapshot()?;
        anyhow::ensure!(checks.sha == tip, "train exact-tip CI SHA changed");
        match crate::forge::judge_commit_checks(&checks, required) {
            crate::forge::CommitCi::Green => return Ok(()),
            crate::forge::CommitCi::Pending(names) => {
                let seconds = elapsed();
                anyhow::ensure!(
                    seconds < limit_secs,
                    "train exact-tip required CI still pending after {limit_secs}s: {}",
                    names.join(", ")
                );
                println!(
                    "  train exact-tip CI pending ({seconds}/{limit_secs}s): {}",
                    names.join(", ")
                );
                sleep();
            }
            state => anyhow::bail!("train exact-tip required CI is not green: {state:?}"),
        }
    }
}

/// Verify the existing objects, without rebuilding timestamped commits. Exact
/// parent/message/tree and patch checks reject hand-edited Built/Car manifests.
fn verify_structure(repo: &Path, train: &Built) -> Result<Vec<Value>> {
    anyhow::ensure!(!train.cars.is_empty(), "empty train proof");
    let mut parent = train.base.clone();
    let mut seen = std::collections::HashSet::new();
    let mut proof = Vec::new();
    for car in &train.cars {
        anyhow::ensure!(seen.insert(car.member.pr), "duplicate train member");
        anyhow::ensure!(
            rev_parse(repo, &car.member.head)? == car.member.head,
            "source head is not full"
        );
        let ancestry = git(repo, &["rev-list", "--parents", "-n", "1", &car.commit])?;
        anyhow::ensure!(
            ancestry == format!("{} {}", car.commit, parent),
            "train car order or parent changed"
        );
        let message = git(repo, &["show", "-s", "--format=%B", &car.commit])?;
        anyhow::ensure!(
            message.trim_end() == car_message(&car.member).trim_end(),
            "train car message changed"
        );
        let fork =
            crate::gate::review_carryover::single_merge_base(repo, &train.base, &car.member.head)
                .map_err(|e| anyhow::anyhow!(e.reason()))?;
        let source_patch = crate::gate::review_carryover::patch_id(repo, &fork, &car.member.head)
            .map_err(|e| anyhow::anyhow!(e.reason()))?;
        let car_patch = crate::gate::review_carryover::patch_id(repo, &parent, &car.commit)
            .map_err(|e| anyhow::anyhow!(e.reason()))?;
        anyhow::ensure!(
            source_patch == car_patch && car_patch == car.patch_id,
            "train patch proof changed"
        );
        let tree = git(
            repo,
            &[
                "merge-tree",
                "--write-tree",
                "--no-messages",
                "--merge-base",
                &fork,
                &parent,
                &car.member.head,
            ],
        )?;
        anyhow::ensure!(
            tree.trim() == git(repo, &["rev-parse", &format!("{}^{{tree}}", car.commit)])?,
            "train car tree changed"
        );
        proof.push(json!({"pr": car.member.pr, "source": car.member.head, "fork": fork,
            "parent": parent, "commit": car.commit, "patch_id": car_patch, "message": message, "tree": tree,
            "release_cover": car.member.release.as_ref().map(|cover| json!({"id":cover.id,"principal":cover.principal}))}));
        parent = car.commit.clone();
    }
    anyhow::ensure!(parent == train.tip, "train tip or car count changed");
    Ok(proof)
}

/// Branch names are hints: queue-work may append a rework suffix which the
/// broad ID parser includes. Only discard an unresolved branch-only hint when
/// explicit metadata supplies a resolvable spec. Ambiguous lookups still refuse,
/// and explicit unknown IDs remain inputs to the strict alias check below.
// trace:BUG-2441 | ai:codex
fn resolved_member_spec_ids(
    root: &Path,
    head_ref: &str,
    title: &str,
    subject: &str,
    body: &str,
) -> Result<Vec<String>> {
    let ids = member_spec_ids(head_ref, title, subject, body);
    let explicit = member_spec_ids("", title, subject, body);
    let store =
        crate::load_store_for_lookup(root).context("cannot resolve train source spec hints")?;
    let mut explicit_resolves = false;
    for id in &explicit {
        explicit_resolves |= store
            .get_requirement_unambiguous(id)
            .map_err(anyhow::Error::msg)?
            .is_some();
    }
    let mut out = Vec::new();
    for id in ids {
        if !explicit.contains(&id)
            && store
                .get_requirement_unambiguous(&id)
                .map_err(anyhow::Error::msg)?
                .is_none()
        {
            // Queue-work appends a numeric rework suffix. Preserve the owning
            // spec's policy gates when that exact stripped hint resolves.
            // trace:TASK-3112 | ai:codex
            if let Some((base, suffix)) = id.rsplit_once('-') {
                if !suffix.is_empty()
                    && suffix.bytes().all(|b| b.is_ascii_digit())
                    && store
                        .get_requirement_unambiguous(base)
                        .map_err(anyhow::Error::msg)?
                        .is_some()
                {
                    if !out.iter().any(|s: &String| s.eq_ignore_ascii_case(base)) {
                        out.push(base.to_string());
                    }
                    continue;
                }
            }
            if explicit_resolves {
                continue;
            }
        }
        if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

fn aliases(root: &Path, ids: &[String]) -> Result<Vec<String>> {
    // trace:BUG-2441 | ai:codex
    anyhow::ensure!(!ids.is_empty(), "train source has no resolvable spec");
    let store =
        crate::load_store_for_lookup(root).context("cannot resolve train source spec aliases")?;
    let mut out = ids.to_vec();
    for id in ids {
        let req = store
            .get_requirement_unambiguous(id)
            .map_err(anyhow::Error::msg)?
            .with_context(|| format!("unresolved train source spec {id}"))?;
        out.extend(
            [
                req.spec_id.clone(),
                req.agreed_id.clone(),
                Some(req.display_id()), // canonical-id: alias set for gate matching
                Some(req.friendly_id()),
            ]
            .into_iter()
            .flatten(),
        );
    }
    out.iter_mut().for_each(|id| *id = id.to_ascii_uppercase());
    out.sort();
    out.dedup();
    Ok(out)
}

// trace:STORY-2492.ac540936 | ai:codex
fn read_bodies(root: &Path, pr: u64, ids: &[String], head: &str) -> Result<Bodies> {
    let main = crate::main_worktree_root_from(root);
    for root in [root, main.as_path()] {
        let store = ledger::store_root(root)?;
        validate_ledger(&store, &format!("PR-{pr}"))?;
        for id in ids {
            validate_ledger(&store, id)?;
        }
    }
    // The member gate accepts this exact-head signed publication. Carry the
    // same body into the mechanical source proof, without requiring a local
    // reviewer receipt or ledger copy. Query only this PR: spec aliases must
    // never project a rider's approval onto the train PR's own head.
    let key = format!("PR-{pr}");
    for site in [root, main.as_path()] {
        if let Some(publication) =
            crate::gate::review_publication::current_at_head(site, &[key.as_str()], head)
        {
            return Ok(vec![(key, serde_json::from_str(&publication.body)?)]);
        }
    }
    corpus::corpus_gate_bodies(&[root, main.as_path()], pr, ids)
        .map_err(anyhow::Error::msg)?
        .into_iter()
        .map(|(key, body)| Ok((key, serde_json::from_str(&body)?)))
        .collect()
}

/// The legacy projection intentionally tolerates torn lines. This authority
/// path must refuse unreadable/invalid ledger or index evidence instead.
fn validate_ledger(root: &Path, key: &str) -> Result<()> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    fn read(path: &Path) -> Result<Option<String>> {
        for parent in path.ancestors().skip(1) {
            if let Ok(meta) = std::fs::symlink_metadata(parent) {
                anyhow::ensure!(
                    !meta.file_type().is_symlink(),
                    "linked train evidence directory"
                );
            }
        }
        let mut file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let meta = file.metadata()?;
        anyhow::ensure!(
            meta.is_file() && meta.nlink() == 1 && meta.len() <= 16 * 1024 * 1024,
            "invalid train evidence file"
        );
        let mut body = String::new();
        file.by_ref()
            .take(16 * 1024 * 1024 + 1)
            .read_to_string(&mut body)?;
        anyhow::ensure!(body.len() <= 16 * 1024 * 1024, "train evidence too large");
        Ok(Some(body))
    }
    let dir = ledger::ledger_dir(root);
    let key = key.to_ascii_uppercase();
    anyhow::ensure!(
        !key.is_empty()
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            && !key.starts_with('.'),
        "invalid train evidence key"
    );
    if ledger::ChangeKey::parse(&key).is_some() {
        if let Some(body) = read(&dir.join(format!("{key}.jsonl")))? {
            for line in body.lines().filter(|l| !l.trim().is_empty()) {
                let event: ledger::GateEvent = serde_json::from_str(line)?;
                anyhow::ensure!(
                    event.v == ledger::event::EVENT_VERSION && event.change == key,
                    "train evidence identity/version changed"
                );
            }
        }
    } else {
        if let Some(body) = read(&dir.join("by-spec").join(format!("{key}.json")))? {
            let index: Value = serde_json::from_str(&body)?;
            for change in index["changes"]
                .as_array()
                .context("invalid train spec index")?
            {
                let change = change.as_str().context("invalid indexed change")?;
                anyhow::ensure!(
                    ledger::ChangeKey::parse(change).is_some(),
                    "invalid indexed change key"
                );
                validate_ledger(root, change)?;
            }
        }
        if let Some(body) = read(&dir.join("by-spec").join(format!("{key}.jsonl")))? {
            for line in body.lines().filter(|l| !l.trim().is_empty()) {
                let event: ledger::GateEvent = serde_json::from_str(line)?;
                anyhow::ensure!(
                    event.v == ledger::event::EVENT_VERSION
                        && event.change.eq_ignore_ascii_case(&key),
                    "invalid unbound train evidence"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/bug2406_train_tests.rs"]
mod bug2406;
