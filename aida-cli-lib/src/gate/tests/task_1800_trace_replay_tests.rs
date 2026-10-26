//! TASK-1800 corpus replay (§4b of the signed-off AC2 sketch). The full
//! replay is local evidence, not CI: the squash-merged PR heads it pins need
//! not exist in CI's clone. Run it with `make task-1800-replay`. TASK-2068:
//! its 18 audited cases also run in CI from a vendored bundle
//! ([`task_1800_audited_cases_replay_from_the_vendored_bundle`]).
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
//!
//! TASK-2067: with the policy classes, `AIDA_TASK1800_LEDGER` (the private
//! exclusion ledger; its entries up to the frozen store commit must hash to
//! the recorded ledger digest) adds the AFTER column: every scoped trace ID is
//! resolved by the production path (`resolve_exclusion_specs`) and judged by
//! the production per-spec rules (`judge_exclusion_spec`) on a store pinned
//! at the frozen store commit, a shared clone with that one ref. The same
//! judgment on the live store is printed as an informational column, never
//! asserted. Named refusals are the frozen ones (named handling is unchanged).
// trace:TASK-1800 | ai:claude
// trace:TASK-2067 | ai:claude
use super::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// The frozen store commit every TASK-1800 classification was read at.
const FROZEN_STORE: &str = "a5dae7145abd11d2480271c466030b12db6e7668";

/// The frozen ledger: entries first seen up to the frozen store commit, whose
/// spec-keyed map must hash (sorted compact JSON) to the recorded digest.
// trace:TASK-2067 | ai:claude
fn frozen_ledger(path: &std::ffi::OsStr) -> BTreeMap<String, crate::gate::mandate::Exclusion> {
    const FROZEN_AT: &str = "2026-10-16T20:56:57Z";
    const DIGEST: &str = "753d1a323b18af13ed73a18f20f9a6f7c6242e3584edf01049897d0dba7289af";
    let frozen_at: chrono::DateTime<chrono::Utc> = FROZEN_AT.parse().unwrap();
    let mut raw = BTreeMap::new();
    let mut ledger = BTreeMap::new();
    for line in std::fs::read_to_string(path).unwrap().lines() {
        let value: Value = serde_json::from_str(line).unwrap();
        let entry: crate::gate::mandate::Exclusion = serde_json::from_value(value.clone()).unwrap();
        if entry.first_seen <= frozen_at {
            raw.entry(entry.spec.clone()).or_insert(value);
            ledger
                .entry(entry.spec.to_ascii_uppercase())
                .or_insert(entry);
        }
    }
    let digest = format!("{:x}", Sha256::digest(serde_json::to_string(&raw).unwrap()));
    assert_eq!(digest, DIGEST, "the ledger is not the frozen one");
    ledger
}

/// A checkout of `commit` in a fresh repository whose only ref is that
/// commit, sharing `repo`'s object database: `--all` sees nothing later.
// trace:TASK-2067 | ai:claude
fn pinned_store(repo: &Path, commit: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let common = git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .unwrap();
    let common = std::path::PathBuf::from(String::from_utf8(common).unwrap().trim());
    let run = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .current_dir(dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    run(&["init", "-q"]);
    std::fs::write(
        dir.path().join(".git/objects/info/alternates"),
        format!("{}\n", common.join("objects").display()),
    )
    .unwrap();
    run(&["update-ref", "refs/heads/store", commit]);
    run(&["symbolic-ref", "HEAD", "refs/heads/store"]);
    run(&["checkout", "-q", "-f", "store"]);
    dir
}

/// One resolved trace ID: how it resolved and its production judgment.
// trace:TASK-2067 | ai:claude
#[derive(Debug)]
struct Resolved {
    kind: &'static str,
    aliases: Vec<String>,
    versions: Option<usize>,
    class: Option<String>,
}

/// Resolve and judge every one of `ids` (as trace-derived IDs) on `store`.
/// One resolution serves every case: each ID's facts are its own.
// trace:TASK-2067 | ai:claude
fn resolve_all(
    store: &Path,
    ids: &BTreeSet<String>,
    ledger: &BTreeMap<String, crate::gate::mandate::Exclusion>,
) -> BTreeMap<String, Resolved> {
    use crate::gate::mandate::MandateRefusal;
    let backend = aida_core::GitBackend::new(store).unwrap();
    let current = aida_core::DatabaseBackend::load(&backend).unwrap();
    let specs =
        crate::gate::mandate_ship::resolve_exclusion_specs(Some(&current), store, &[], ids.clone());
    specs
        .into_iter()
        .map(|spec| {
            let versions = spec.history.as_ref().ok().map(Vec::len);
            let current = spec.current.as_ref().map(|r| {
                r.spec_id
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&spec.id))
            });
            let kind = resolution_kind(current, versions, spec.aliases.is_empty());
            let class = match crate::gate::mandate::judge_exclusion_spec(&spec, ledger, false) {
                Ok(()) => None,
                Err(MandateRefusal::AlwaysHuman { class, .. }) => Some(class),
                Err(MandateRefusal::Excluded { why, .. }) => Some(why),
                Err(MandateRefusal::HistoryUnreadable { why, .. }) => {
                    Some(format!("history unreadable: {why}"))
                }
                Err(other) => Some(format!("{other:?}")),
            };
            let resolved = Resolved {
                kind,
                aliases: spec.aliases.clone(),
                versions,
                class,
            };
            (spec.id, resolved)
        })
        .collect()
}

/// The report label for how an ID resolved: `current` is `Some(true)` when
/// the current record is the ID's own, `Some(false)` when it is found only
/// through a recorded alias.
// trace:TASK-2238 | ai:claude
fn resolution_kind(
    current: Option<bool>,
    versions: Option<usize>,
    no_aliases: bool,
) -> &'static str {
    match (current, versions) {
        (Some(true), _) => "current",
        (Some(false), _) => "recorded alias",
        (None, None) => "history unreadable",
        (None, Some(0)) if no_aliases => "dangling",
        // TASK-2233: an alias was found but no version under it was.
        (None, Some(0)) => "alias with no versions",
        (None, Some(_)) => "deleted with history",
    }
}

/// TASK-2233: an ID with aliases but no versions is not "deleted with
/// history".
// trace:TASK-2238 | ai:claude
#[test]
fn task_2238_resolution_kind_labels_an_alias_with_no_versions() {
    assert_eq!(
        resolution_kind(None, Some(0), false),
        "alias with no versions"
    );
    assert_eq!(resolution_kind(None, Some(0), true), "dangling");
    assert_eq!(
        resolution_kind(None, Some(2), false),
        "deleted with history"
    );
    assert_eq!(resolution_kind(None, None, false), "history unreadable");
    assert_eq!(resolution_kind(Some(true), Some(0), true), "current");
    assert_eq!(resolution_kind(Some(false), None, false), "recorded alias");
}

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
    let ledger = std::env::var_os("AIDA_TASK1800_LEDGER")
        .filter(|p| !p.is_empty() && policy.is_some())
        .map(|path| frozen_ledger(&path));
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
    // TASK-2067: the AFTER column, production resolution on the pinned store,
    // and the live store as an informational column.
    let resolution = ledger.as_ref().map(|ledger| {
        let ids: BTreeSet<String> = scoped.values().flatten().cloned().collect();
        let pinned = pinned_store(repo, FROZEN_STORE);
        let live = crate::store_sync::detect_distributed_store_from(repo);
        let columns = [
            ("pinned", Some(resolve_all(pinned.path(), &ids, ledger))),
            ("live", live.map(|live| resolve_all(&live, &ids, ledger))),
        ];
        let policy = policy.as_ref().unwrap();
        let mut report = serde_json::Map::new();
        for (name, column) in &columns {
            let Some(column) = column else { continue };
            let mut total = 0;
            let mut changed = Vec::new();
            for case in &mut cases {
                let pr = case["pr"].as_u64().unwrap();
                let row = frozen["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["pr"] == pr)
                    .unwrap();
                let named = !row["classification"]["scoped_ordinary"]["named_blocker"].is_null();
                let blockers: Vec<&String> = scoped[&pr]
                    .iter()
                    .filter(|id| column[*id].class.is_some())
                    .collect();
                let refused = named || !blockers.is_empty();
                total += usize::from(refused);
                case[format!("refused_after_{name}")] = json!(refused);
                if Some(refused) != case["refused"].as_bool() {
                    changed.push(json!({"pr": pr, "before": case["refused"], "after": refused, "blockers": blockers}));
                }
            }
            let ids: BTreeMap<&String, Value> = column
                .iter()
                .filter(|(id, r)| policy.get(*id).cloned().flatten() != r.class)
                .map(|(id, r)| {
                    (id, json!({"kind": r.kind, "aliases": r.aliases, "versions": r.versions,
                        "before": policy.get(id).cloned().flatten(), "after": r.class}))
                })
                .collect();
            println!("refusals over 50 after, {name} store: {total}");
            report.insert(
                name.to_string(),
                json!({"total": total, "changed_cases": changed, "changed_ids": ids}),
            );
        }
        (report, columns)
    });
    // The 18 audited cases: blob drift, then the zero-loss gate.
    audit_gate(
        repo,
        &audit,
        &scoped,
        |row, side| row[side].as_str().unwrap().to_owned(),
        &mut drift,
        &mut lost,
    );
    // TASK-2067: how each audited unresolved ID now resolves on the pinned
    // store. None may still refuse for an unreadable or missing history.
    let mut unresolved = BTreeMap::new();
    if let Some((_, columns)) = &resolution {
        let pinned = columns[0].1.as_ref().unwrap();
        for row in audit["rows"].as_array().unwrap() {
            for blocker in row["blockers"].as_array().unwrap() {
                let id = blocker["id"].as_str().unwrap();
                if blocker["judgment"] != "unresolved" || unresolved.contains_key(id) {
                    continue;
                }
                let Some(r) = pinned.get(id) else { continue };
                if r.class
                    .as_deref()
                    .is_some_and(|c| c.starts_with("history unreadable"))
                {
                    lost.push(format!("{id} still refuses: {:?}", r.class));
                }
                unresolved.insert(
                    id.to_owned(),
                    json!({"kind": r.kind, "aliases": r.aliases, "versions": r.versions, "after": r.class}),
                );
            }
        }
    }
    let report = json!({
        "cases": cases,
        "refusals": policy.as_ref().map(|_| json!({"named": named_refused, "total": refused})),
        "resolution": resolution.as_ref().map(|(report, _)| report),
        "unresolved_ids": unresolved,
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

/// The 18 audited cases: each audited occurrence's blob still hashes as the
/// audit recorded it (drift), and no (case, ID) pair the audit judged
/// true-touch or unresolved in scoped mode is missing from `scoped` (the
/// zero-loss gate). `commit(row, side)` names the case's base or head.
// trace:TASK-1800 trace:TASK-2067 | ai:claude
fn audit_gate(
    repo: &Path,
    audit: &Value,
    scoped: &BTreeMap<u64, BTreeSet<String>>,
    commit: impl Fn(&Value, &str) -> String,
    drift: &mut Vec<String>,
    lost: &mut Vec<String>,
) {
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
                    commit(row, side),
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
}

/// TASK-2068: the CI half of the replay. The vendored bundle
/// (`tests/fixtures/task-1800-replay/`, built by
/// `scripts/task-1800-replay-fixture.py`) holds a synthetic base and head per
/// audited case with exactly the real diff, so the production collector must
/// reproduce each recorded scoped digest, fallback set and changed-path list,
/// every audited blob, and the zero-loss gate. Advisor C1: under
/// `diff.renames` false and copies, the collector's one git diff call
/// ([`diff_tree_args`]) must return the same bytes, so its evidence cannot
/// depend on the rename config (the collector itself runs once: it is slow).
// trace:TASK-2067 | ai:claude
#[test]
fn task_1800_audited_cases_replay_from_the_vendored_bundle() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let frozen = load(repo, "docs/testing/task-1800-trace-exclusions.json");
    let audit = load(repo, "docs/testing/task-1800-semantic-audit.json");
    let bundle = repo.join("tests/fixtures/task-1800-replay/audited-cases.bundle");
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .current_dir(dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    run(&["init", "-q", "--bare"]);
    run(&[
        "fetch",
        "-q",
        bundle.to_str().unwrap(),
        "refs/task-1800/*:refs/task-1800/*",
    ]);
    let commit = |row: &Value, side: &str| {
        let name = format!("refs/task-1800/{}/{side}^{{commit}}", row["pr"]);
        let sha = git(dir.path(), &["rev-parse", "--verify", &name]).unwrap();
        String::from_utf8(sha).unwrap().trim().to_owned()
    };
    let rows = audit["rows"].as_array().unwrap();
    let listings = || -> Vec<Vec<u8>> {
        rows.iter()
            .map(|row| {
                let (base, head) = (commit(row, "base"), commit(row, "head"));
                git(dir.path(), &diff_tree_args(&base, &head)).unwrap()
            })
            .collect()
    };
    let unset = listings();
    for renames in ["false", "copies"] {
        run(&["config", "diff.renames", renames]);
        assert!(
            listings() == unset,
            "diff.renames={renames} changes the diff"
        );
    }
    run(&["config", "--unset", "diff.renames"]);
    let (mut drift, mut lost) = (Vec::new(), Vec::new());
    let mut scoped = BTreeMap::new();
    for row in rows {
        let pr = row["pr"].as_u64().unwrap();
        let record = frozen["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["pr"] == pr)
            .unwrap_or_else(|| panic!("PR {pr} is not a frozen case"));
        let items = collect(
            dir.path(),
            &commit(row, "base"),
            &commit(row, "head"),
            Scope::ChangedItems,
        )
        .unwrap();
        if digest(&items.ids) != record["scoped"]["ids_sha256"].as_str().unwrap() {
            drift.push(format!(
                "PR {pr}: scoped set differs from the recorded digest"
            ));
        }
        if items.full_file_fallbacks != strings(&record["scoped"]["fallbacks"]) {
            drift.push(format!(
                "PR {pr}: scoped fallback paths differ from the record"
            ));
        }
        let audited: BTreeSet<String> = row["changed_paths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["path"].as_str().unwrap().to_owned())
            .collect();
        if items.paths != audited {
            drift.push(format!("PR {pr}: changed paths differ from the audit"));
        }
        scoped.insert(pr, items.ids);
    }
    audit_gate(dir.path(), &audit, &scoped, commit, &mut drift, &mut lost);
    assert_eq!(scoped.len(), 18, "the audit covers 18 cases");
    assert!(drift.is_empty(), "{drift:#?}");
    assert!(lost.is_empty(), "{lost:#?}");
}
