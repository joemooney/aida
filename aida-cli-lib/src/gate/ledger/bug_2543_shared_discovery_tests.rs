//! BUG-2543 / BUG-2551: a verdict listing or pull's wait reconciliation
//! shares ONE gate-ledger discovery across its keys
//! ([`checked::DiscoveryScope`]), while every key's events are still read
//! fresh, and the shared path answers and refuses exactly as a fresh
//! discovery does.
// trace:BUG-2543 BUG-2551 | ai:claude
// mandate-gate

use super::*;

const FIRST: u64 = 3651;
const SECOND: u64 = 3764;

/// Discoveries since the last reset (each builds one ownership index).
fn discoveries() -> usize {
    project::OWNERSHIP_WORK.with(|work| work.get()).0
}

fn reset() {
    project::OWNERSHIP_WORK.with(|work| work.set((0, 0)));
}

/// A ledger whose PR-3651 holds a foreign review copy that only the
/// discovery's ownership index (the phase-3 witness on PR-3764) withholds,
/// plus a handful of ordinary change ledgers. Returns the old refusal that a
/// correct read of PR-3651 answers.
fn miskeyed_fixture(root: &Path) -> JsonObj {
    let spec = GateSpec::resolve(root, "BUG-2492");
    let old = record("request_changes", &ts(1), Some(&attestation("reviewer")));
    store::append_one(
        root,
        event::verdict_event(root, ChangeKey::pr(FIRST), &old, Some(&spec)),
    )
    .unwrap();
    let mut foreign = record("approved", &ts(2), Some(&attestation("reviewer")));
    foreign.insert("reviewed_sha".into(), serde_json::json!(OTHER_HEAD));
    store::append_one(
        root,
        event::verdict_event(root, ChangeKey::pr(FIRST), &foreign, Some(&spec)),
    )
    .unwrap();
    foreign.insert("mode".into(), serde_json::json!(project::PHASE3_MODE));
    store::append_one(
        root,
        event::verdict_event(root, ChangeKey::pr(SECOND), &foreign, Some(&spec)),
    )
    .unwrap();
    for n in 1..=6u64 {
        let spec = GateSpec::with_key(&format!("BUG-{n}"), None);
        let verdict = if n % 2 == 0 { "approved" } else { "rejected" };
        let rec = record(verdict, &ts(n as i64 + 2), Some(&attestation("reviewer")));
        store::append_one(
            root,
            event::verdict_event(root, ChangeKey::pr(n), &rec, Some(&spec)),
        )
        .unwrap();
    }
    old
}

fn keys() -> Vec<String> {
    let mut keys: Vec<String> = (1..=6)
        .flat_map(|n| [format!("PR-{n}"), format!("BUG-{n}")])
        .collect();
    keys.extend([
        format!("PR-{FIRST}"),
        format!("PR-{SECOND}"),
        "BUG-2492".into(),
    ]);
    keys
}

/// Everything the single-key readers answer for `keys` under `root`.
fn answers(root: &Path, keys: &[String]) -> String {
    let mut out = Vec::new();
    for key in keys {
        let events = ChangeKey::parse(key)
            .map(|change| project::events(root, change))
            .unwrap_or_default();
        out.push(serde_json::json!({
            "key": key,
            "events": serde_json::to_value(&events).unwrap(),
            "newest": project::newest_verdict_event(root, key)
                .and_then(|e| e.verdict_record().cloned()),
            "current": rv::current_verdict(root, key).map(|c| c.body),
        }));
    }
    serde_json::to_string(&out).unwrap()
}

// The shared discovery answers exactly what fresh discovery answers, with
// one discovery for every key instead of one (or more) per key.
// trace:BUG-2543 | ai:claude
#[test]
fn bug_2543_shared_discovery_equals_fresh_discovery() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let old = miskeyed_fixture(&root);
    let keys = keys();

    reset();
    let fresh = answers(&root, &keys);
    assert!(
        discoveries() > keys.len(),
        "outside a scope every read discovers afresh"
    );
    assert_eq!(
        project::newest_verdict_event(&root, &format!("PR-{FIRST}"))
            .unwrap()
            .verdict_record(),
        Some(&old),
        "the fixture's foreign copy is withheld by ownership"
    );

    reset();
    let shared = {
        let _scope = checked::DiscoveryScope::enter();
        answers(&root, &keys)
    };
    assert_eq!(discoveries(), 1, "one discovery for {} keys", keys.len());
    assert_eq!(shared, fresh);

    reset();
    let listed = rv::active_verdicts_for(&root, keys.clone());
    assert_eq!(discoveries(), 1, "a verdict listing discovers once");
    let mut single: Vec<(String, rv::RecordedVerdict)> = keys
        .iter()
        .filter_map(|key| {
            let body = rv::current_verdict(&root, key)?.body;
            Some((key.clone(), rv::parse_recorded_verdict(&body)?))
        })
        .collect();
    single.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(format!("{listed:?}"), format!("{single:?}"));
}

/// A change to the ledger after the scope's discovery, applied to `root`.
#[derive(Debug, Clone, Copy)]
enum Mutation {
    HardLink,
    Symlink,
    Fifo,
    LinkedLedgerDir,
    ForeignAppend,
}

fn mkfifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: `c` is a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
}

fn mutate(root: &Path, change: ChangeKey, how: Mutation) {
    let file = store::ledger_file(root, change);
    let aside = root
        .join(".aida")
        .join(format!("aside-{}.jsonl", change.as_key()));
    match how {
        Mutation::HardLink => std::fs::hard_link(&file, &aside).unwrap(),
        Mutation::Symlink => {
            std::fs::rename(&file, &aside).unwrap();
            std::os::unix::fs::symlink(&aside, &file).unwrap();
        }
        Mutation::Fifo => {
            std::fs::rename(&file, &aside).unwrap();
            mkfifo(&file);
        }
        Mutation::LinkedLedgerDir => {
            let dir = ledger_dir(root);
            let real = root.join(".aida").join("gate-ledger-real");
            std::fs::rename(&dir, &real).unwrap();
            std::os::unix::fs::symlink(&real, &dir).unwrap();
        }
        Mutation::ForeignAppend => {
            // Another process's append: raw bytes, not through this process.
            let body = std::fs::read_to_string(&file).unwrap();
            let mut next: serde_json::Value =
                serde_json::from_str(body.lines().last().unwrap()).unwrap();
            next["seq"] = serde_json::json!(next["seq"].as_u64().unwrap() + 1);
            let mut out = std::fs::OpenOptions::new()
                .append(true)
                .open(&file)
                .unwrap();
            std::io::Write::write_all(&mut out, format!("{next}\n").as_bytes()).unwrap();
        }
    }
}

/// Run `f` on a fresh thread (a scope is per thread), failing instead of
/// hanging the suite if it blocks (a FIFO opened for a blocking read).
fn bounded<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(std::time::Duration::from_secs(30))
        .expect("a gate-ledger read hung")
}

// Hard-linked, symlinked and FIFO ledgers, a linked ledger directory and a
// foreign append, each made after the scope discovered the ledger, are
// refused (or answered) on the shared path exactly as fresh discovery does,
// on the key's own ledger and on an unrelated one, and nothing blocks.
// trace:BUG-2543 | ai:claude
#[test]
fn bug_2543_shared_discovery_refuses_links_and_fifos_as_fresh_discovery() {
    let _env = uncontained();
    let cases = [
        (Mutation::HardLink, FIRST),
        (Mutation::HardLink, 1),
        (Mutation::Symlink, FIRST),
        (Mutation::Symlink, 1),
        (Mutation::Fifo, FIRST),
        (Mutation::Fifo, 1),
        (Mutation::LinkedLedgerDir, FIRST),
        (Mutation::ForeignAppend, SECOND),
    ];
    for (how, target) in cases {
        let (tmp, root) = root();
        let old = miskeyed_fixture(&root);
        let keys = keys();
        let (listed, rediscovered) = {
            let root = root.clone();
            let keys = keys.clone();
            bounded(move || {
                let _scope = checked::DiscoveryScope::enter();
                reset();
                let _ = answers(&root, &keys);
                assert_eq!(discoveries(), 1, "{how:?}");
                mutate(&root, ChangeKey::pr(target), how);
                reset();
                let listed = answers(&root, &keys);
                (listed, discoveries())
            })
        };
        // A linked ledger directory refuses before any ledger is read, so
        // its rediscovery builds no ownership index to count.
        assert!(
            rediscovered >= 1 || matches!(how, Mutation::LinkedLedgerDir),
            "{how:?} on PR-{target} must rediscover"
        );
        let fresh = {
            let root = root.clone();
            let keys = keys.clone();
            bounded(move || answers(&root, &keys))
        };
        assert_eq!(listed, fresh, "{how:?} on PR-{target}");
        let newest = {
            let root = root.clone();
            bounded(move || project::newest_verdict_event(&root, &format!("PR-{FIRST}")))
        };
        match how {
            // A foreign append that is not a witness changes nothing PR-3651
            // answers; the shared path still rediscovered rather than reuse.
            Mutation::ForeignAppend => {
                assert_eq!(newest.unwrap().verdict_record(), Some(&old));
            }
            // Fresh discovery refuses: every review copy is withheld, the
            // foreign approval above all.
            _ => assert!(
                newest.is_none(),
                "{how:?} on PR-{target}: refused discovery must fail closed"
            ),
        }
        drop(tmp);
    }
}

// Pull's wait reconciliation over N keys discovers the ledger once.
// trace:BUG-2551 | ai:claude
#[test]
fn bug_2551_reconcile_discovers_once_for_n_keys() {
    let (_tmp, root) = root();
    let _env = uncontained();
    miskeyed_fixture(&root);
    const WAITS: u64 = 8;
    let keys: Vec<String> = (101..101 + WAITS).map(|n| format!("PR-{n}")).collect();
    for key in &keys {
        rv::record_verdict(
            &root,
            key,
            Some("waiting"),
            Some(HEAD),
            None,
            Some("waiting for batch CI"),
            &[],
            "reviewer",
        )
        .unwrap();
    }
    let all = rv::verdict_keys(&root);
    assert!(all.len() >= WAITS as usize, "{all:?}");
    reset();
    crate::review_waiting::reconcile_waiting_verdicts_with(&root, None, |_| None);
    assert_eq!(discoveries(), 1, "one discovery for {} keys", all.len());
    for key in &keys {
        assert!(!rv::read_recorded_verdict(&root, key).unwrap().is_closed());
    }

    // Closing waits appends to the ledger; each append changes it, so the
    // next read rediscovers (no own-append fold). A closure writes and
    // re-reads a few times, so the cost is a small constant per closure,
    // never one per key read. trace:BUG-2551 | ai:claude
    reset();
    crate::review_waiting::reconcile_waiting_verdicts_with(&root, None, |_| {
        Some((crate::forge::ChangeState::Merged, HEAD.into()))
    });
    for key in &keys {
        assert!(
            rv::read_recorded_verdict(&root, key).unwrap().is_closed(),
            "{key}"
        );
    }
    assert!(
        discoveries() <= 1 + 4 * WAITS as usize,
        "{} discoveries for {WAITS} closures",
        discoveries()
    );
}

// Timing of a listing over a corpus of many change ledgers, fresh (one
// discovery per read) against shared. Run on demand:
// cargo test --profile agent -p aida-cli-lib --lib -- --ignored --nocapture bug_2543_bench
// trace:BUG-2543 | ai:claude
#[test]
#[ignore = "timing bench, run on demand"]
fn bug_2543_bench_listing_fresh_vs_shared() {
    let (_tmp, root) = root();
    let _env = uncontained();
    let ledgers: u64 = 400;
    for n in 1..=ledgers {
        let spec = GateSpec::with_key(&format!("BUG-{n}"), None);
        let mut rec = record(
            "request_changes",
            &ts(n as i64),
            Some(&attestation("reviewer")),
        );
        rec.insert(
            "findings".into(),
            serde_json::json!(["x".repeat(16 * 1024)]),
        );
        store::append_one(
            &root,
            event::verdict_event(&root, ChangeKey::pr(n), &rec, Some(&spec)),
        )
        .unwrap();
    }
    let keys: Vec<String> = (1..=ledgers).map(|n| format!("PR-{n}")).collect();
    let started = std::time::Instant::now();
    let fresh: Vec<_> = keys
        .iter()
        .map(|key| project::newest_verdict_event(&root, key))
        .collect();
    let fresh_elapsed = started.elapsed();
    reset();
    let started = std::time::Instant::now();
    let shared: Vec<_> = {
        let _scope = checked::DiscoveryScope::enter();
        keys.iter()
            .map(|key| project::newest_verdict_event(&root, key))
            .collect()
    };
    let shared_elapsed = started.elapsed();
    assert_eq!(fresh, shared);
    assert_eq!(discoveries(), 1);
    let started = std::time::Instant::now();
    crate::review_waiting::reconcile_waiting_verdicts_with(&root, None, |_| None);
    let reconcile_elapsed = started.elapsed();
    eprintln!(
        "BUG-2543 bench: {ledgers} ledgers/keys: fresh {fresh_elapsed:?}, shared {shared_elapsed:?}, reconcile (shared) {reconcile_elapsed:?}"
    );
}
