//! TASK-1800 corpus replay (§4b of the signed-off AC2 sketch). Local
//! evidence, not CI: the squash-merged PR heads it pins need not exist in
//! CI's clone. Run it with `make task-1800-replay`.
//!
//! It replays the production collector (`ChangedItems`) and the full file on
//! the 50 frozen endpoints of `docs/testing/task-1800-trace-exclusions.json`.
//! It checks that both still reproduce the recorded digests and fallback
//! sets (corpus/collector drift) and that the 18-case audit's blob hashes
//! match. It fails when any (case, ID) pair the audit judged true-touch or
//! unresolved in scoped mode is no longer collected. A change to trace
//! attribution must keep that gate and report its before/after here.
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
    let mut cases = Vec::new();
    let mut scoped: BTreeMap<u64, BTreeSet<String>> = BTreeMap::new();
    let (mut refused, mut named_refused) = (0, 0);
    for row in frozen["rows"].as_array().unwrap() {
        let pr = row["pr"].as_u64().unwrap();
        let base = row["base"].as_str().unwrap();
        let head = row["head"].as_str().unwrap();
        let full = collect(repo, base, head, Scope::FullFile).unwrap();
        let items = collect(repo, base, head, Scope::ChangedItems).unwrap();
        if digest(&full.ids) != row["full"]["ids_sha256"].as_str().unwrap() {
            drift.push(format!(
                "PR {pr}: full-file set differs from the recorded digest"
            ));
        }
        if digest(&items.ids) != row["scoped"]["ids_sha256"].as_str().unwrap() {
            drift.push(format!(
                "PR {pr}: scoped set differs from the recorded digest"
            ));
        }
        if items.full_file_fallbacks != strings(&row["scoped"]["fallbacks"]) {
            drift.push(format!(
                "PR {pr}: scoped fallback paths differ from the record"
            ));
        }
        let named = &row["classification"]["scoped_ordinary"]["named_blocker"];
        let is_refused = policy.as_ref().map(|policy| {
            !named.is_null()
                || items.ids.iter().any(|id| {
                    policy
                        .get(id)
                        .unwrap_or_else(|| panic!("{id} has no frozen policy class"))
                        .is_some()
                })
        });
        if let Some(is_refused) = is_refused {
            refused += usize::from(is_refused);
            named_refused += usize::from(!named.is_null());
        }
        cases.push(json!({
            "pr": pr,
            "full_ids": full.ids.len(),
            "scoped_ids": items.ids.len(),
            "scoped_fallbacks": items.full_file_fallbacks.len(),
            "refused": is_refused,
        }));
        scoped.insert(pr, items.ids);
    }
    // The 18 audited cases: blob drift, then the zero-loss gate.
    for row in audit["rows"].as_array().unwrap() {
        let pr = row["pr"].as_u64().unwrap();
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
            let in_scope = strings(&blocker["modes"]).contains("scoped");
            if in_scope && judgment != "incidental" && !scoped[&pr].contains(id) {
                lost.push(format!("PR {pr}: {judgment} {id}"));
            }
        }
    }
    let report = json!({
        "cases": cases,
        "refusals": policy.as_ref().map(|_| json!({"named": named_refused, "total": refused})),
        "lost": lost,
        "drift": drift,
    });
    if let Some(out) = std::env::var_os("AIDA_TASK1800_REPLAY_OUT").filter(|p| !p.is_empty()) {
        std::fs::write(out, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    if policy.is_some() {
        println!("refusals over 50: {refused} ({named_refused} named)");
    }
    assert!(drift.is_empty(), "corpus or collector drift: {drift:#?}");
    assert!(
        lost.is_empty(),
        "true-touch/unresolved pairs lost: {lost:#?}"
    );
}
