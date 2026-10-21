//! TASK-1800 AC2 corpus replay (§4b of the signed-off sketch). Local
//! evidence, not CI: the squash-merged PR heads it pins need not exist in
//! CI's clone. Run it with `make task-1800-replay`.
//!
//! It replays the fn/impl collector (`ChangedItemsV1`), the item-level
//! collector and the full file on the 50 frozen endpoints of
//! `docs/testing/task-1800-trace-exclusions.json`, checks that the old and
//! full sets still reproduce the recorded digests (corpus/collector drift),
//! checks the 18-case audit's blob hashes, and fails when the new collector
//! drops any (case, ID) pair the audit judged true-touch or unresolved that
//! the old scoped collector kept.
//!
//! Case-level refusals need the frozen per-ID policy classes, which are local
//! relay evidence and not published; pass their path in
//! `AIDA_TASK1800_POLICY_CLASSES` to add them. `AIDA_TASK1800_REPLAY_OUT`
//! names a file for the JSON report.
// trace:TASK-1800 | ai:claude
use super::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn digest(ids: &BTreeSet<String>) -> String {
    let sorted: Vec<&String> = ids.iter().collect();
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_string(&sorted).unwrap())
    )
}

fn load(repo: &Path, rel: &str) -> Value {
    serde_json::from_slice(&std::fs::read(repo.join(rel)).unwrap()).unwrap()
}

fn strings(v: &Value) -> BTreeSet<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect()
}

#[test]
#[ignore = "local corpus replay: make task-1800-replay"]
fn task_1800_corpus_replay() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let frozen = load(repo, "docs/testing/task-1800-trace-exclusions.json");
    let audit = load(repo, "docs/testing/task-1800-semantic-audit.json");
    let policy: Option<BTreeMap<String, Option<String>>> =
        std::env::var_os("AIDA_TASK1800_POLICY_CLASSES")
            .filter(|p| !p.is_empty())
            .map(|path| serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap());
    let mut drift = Vec::new();
    let mut lost = Vec::new();
    let mut invariant = Vec::new();
    let mut cases = Vec::new();
    let mut evidence: BTreeMap<u64, (Evidence, Evidence)> = BTreeMap::new();
    let (mut old_refused, mut new_refused, mut named_refused) = (0, 0, 0);
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    // Paths the fn/impl collector scoped (no fallback) whose set the new
    // collector grows: the closure widening evidence that was already scoped.
    let mut grown: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for row in frozen["rows"].as_array().unwrap() {
        let pr = row["pr"].as_u64().unwrap();
        let base = row["base"].as_str().unwrap();
        let head = row["head"].as_str().unwrap();
        let full = collect(repo, base, head, Scope::FullFile).unwrap();
        let old = collect(repo, base, head, Scope::ChangedItemsV1).unwrap();
        let new = collect(repo, base, head, Scope::ChangedItems).unwrap();
        if digest(&full.ids) != row["full"]["ids_sha256"].as_str().unwrap() {
            drift.push(format!(
                "PR {pr}: full-file set differs from the recorded digest"
            ));
        }
        if digest(&old.ids) != row["scoped"]["ids_sha256"].as_str().unwrap() {
            drift.push(format!(
                "PR {pr}: fn/impl scoped set differs from the recorded digest"
            ));
        }
        if old.full_file_fallbacks != strings(&row["scoped"]["fallbacks"]) {
            drift.push(format!(
                "PR {pr}: fn/impl fallback paths differ from the record"
            ));
        }
        for (path, (new_ids, new_why)) in &new.per_path {
            let (full_ids, _) = &full.per_path[path];
            let (old_ids, old_why) = &old.per_path[path];
            if !new_ids.is_subset(full_ids) {
                invariant.push(format!("PR {pr} {path}: new IDs outside the full file"));
            }
            if old_why.is_empty() && !old_ids.is_subset(new_ids) {
                invariant.push(format!("PR {pr} {path}: an old scoped ID was dropped"));
            }
            if old_why.is_empty() && new_ids.len() > old_ids.len() {
                grown.entry(pr).or_default().push(path.clone());
            }
            for why in new_why {
                *reasons.entry(format!("{why:?}")).or_default() += 1;
            }
        }
        let named = &row["classification"]["scoped_ordinary"]["named_blocker"];
        let refused = |ids: &BTreeSet<String>| {
            policy.as_ref().map(|policy| {
                !named.is_null()
                    || ids.iter().any(|id| {
                        policy
                            .get(id)
                            .unwrap_or_else(|| panic!("{id} has no frozen policy class"))
                            .is_some()
                    })
            })
        };
        let (was, now) = (refused(&old.ids), refused(&new.ids));
        if let (Some(was), Some(now)) = (was, now) {
            old_refused += usize::from(was);
            new_refused += usize::from(now);
            named_refused += usize::from(!named.is_null());
        }
        cases.push(json!({
            "pr": pr,
            "base": base,
            "head": head,
            "full_ids": full.ids.len(),
            "old_ids": old.ids.len(),
            "new_ids": new.ids.len(),
            "new_ids_sha256": digest(&new.ids),
            "old_fallbacks": old.full_file_fallbacks,
            "new_fallbacks": new.per_path.iter()
                .filter(|(_, (_, why))| !why.is_empty())
                .map(|(path, (_, why))| (path.clone(), why.iter().map(|w| format!("{w:?}")).collect::<Vec<_>>()))
                .collect::<BTreeMap<_, _>>(),
            "named_refusal": !named.is_null(),
            "old_refused": was,
            "new_refused": now,
        }));
        evidence.insert(pr, (old, new));
    }
    // The 18 audited cases: blob drift, then the zero-loss gate.
    let mut pairs: BTreeMap<String, [usize; 3]> = BTreeMap::new();
    let mut per_case: BTreeMap<u64, BTreeMap<String, [usize; 3]>> = BTreeMap::new();
    for row in audit["rows"].as_array().unwrap() {
        let pr = row["pr"].as_u64().unwrap();
        let (old, new) = &evidence[&pr];
        for blocker in row["blockers"].as_array().unwrap() {
            for occurrence in blocker["occurrences"].as_array().unwrap() {
                let side = if occurrence["side"] == "base" {
                    "base"
                } else {
                    "head"
                };
                let spec = format!(
                    "{}:{}",
                    row[side].as_str().unwrap(),
                    occurrence["path"].as_str().unwrap()
                );
                let blob = git(repo, &["cat-file", "blob", &spec]).unwrap();
                if format!("{:x}", Sha256::digest(&blob)) != occurrence["blob_sha256"] {
                    drift.push(format!("PR {pr}: blob {spec} differs from the audit"));
                }
            }
            let id = blocker["id"].as_str().unwrap();
            let judgment = blocker["judgment"].as_str().unwrap();
            let scoped = strings(&blocker["modes"]).contains("scoped");
            let key = format!(
                "{}/{}",
                if scoped { "scoped" } else { "full-only" },
                judgment
            );
            for counts in [
                pairs.entry(key.clone()).or_default(),
                per_case.entry(pr).or_default().entry(key).or_default(),
            ] {
                counts[0] += 1;
                counts[1] += usize::from(old.ids.contains(id));
                counts[2] += usize::from(new.ids.contains(id));
            }
            if scoped && judgment != "incidental" && !new.ids.contains(id) {
                lost.push(format!("PR {pr}: {judgment} {id}"));
            }
        }
    }
    let (old_paths, new_paths): (usize, usize) = evidence.values().fold((0, 0), |acc, (o, n)| {
        (
            acc.0 + o.full_file_fallbacks.len(),
            acc.1 + n.full_file_fallbacks.len(),
        )
    });
    let report = json!({
        "cases": cases,
        "audit_pairs": pairs.iter().map(|(k, [total, old, new])| (k.clone(), json!({"total": total, "old_kept": old, "new_kept": new}))).collect::<BTreeMap<_, _>>(),
        "audit_cases": per_case,
        "fallback_case_paths": {"old": old_paths, "new": new_paths},
        "grown_scoped_paths": grown,
        "new_fallback_reasons": reasons,
        "refusals": policy.as_ref().map(|_| json!({"named": named_refused, "old": old_refused, "new": new_refused})),
        "lost": lost,
        "drift": drift,
        "invariant": invariant,
    });
    if let Some(out) = std::env::var_os("AIDA_TASK1800_REPLAY_OUT").filter(|p| !p.is_empty()) {
        std::fs::write(out, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    println!("audit pairs (total / kept by fn-impl / kept by item-level):");
    for (k, [total, old, new]) in &pairs {
        println!("  {k}: {total} / {old} / {new}");
    }
    println!("fallback case/paths: {old_paths} -> {new_paths}; new reasons {reasons:?}");
    println!(
        "scoped paths grown: {} paths in {} cases",
        grown.values().map(Vec::len).sum::<usize>(),
        grown.len()
    );
    if policy.is_some() {
        println!("refusals over 50: {old_refused} -> {new_refused} ({named_refused} named)");
    }
    assert!(drift.is_empty(), "corpus or collector drift: {drift:#?}");
    assert!(
        invariant.is_empty(),
        "subset/superset invariant: {invariant:#?}"
    );
    assert!(
        lost.is_empty(),
        "true-touch/unresolved pairs lost: {lost:#?}"
    );
}
