//! TASK-1945 S3b: the author's findings mail for a blocking verdict and the
//! advisor's round-cap wake, each delivered once. Verdicts are real files
//! read by the production history reader; no LLM, no cargo, no network.
// trace:TASK-1945 | ai:claude
use super::ready::{Approval, Evidence};
use super::*;

const REPO: &str = "github:github.com/owner/repo";

fn sha(n: u64) -> String {
    format!("{n:040x}")
}

/// A store whose roster maps the forge login `writer` to `Writer`.
fn fixture() -> (tempfile::TempDir, Store) {
    let temp = tempfile::tempdir().unwrap();
    let registry = temp.path().join(".aida-store").join("registry");
    std::fs::create_dir_all(&registry).unwrap();
    std::fs::write(registry.join("team.toml"), "[members]\nWriter = []\n").unwrap();
    let store = Store::open(temp.path(), REPO).unwrap();
    (temp, store)
}

fn change(head: u64) -> forge::Change {
    forge::Change {
        id: 7,
        head: sha(head),
        base: "main".into(),
        author: "writer".into(),
        gitlab: false,
        overlap: Some(false),
        behind: Some(0),
    }
}

fn verdict(head: u64, word: &str) -> serde_json::Value {
    serde_json::json!({
        "reviewed_sha": sha(head),
        "recorded_by": "reviewer seat",
        "verdict": word,
        "findings": ["unwrap on a forge error", "no test for the retry"],
    })
}

/// Write a verdict file under `.aida/review-verdicts/`.
fn write(root: &Path, name: &str, value: &serde_json::Value) {
    let path = root.join(".aida/review-verdicts").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
}

/// Never ready, so no ready wake joins the mailbox.
struct Unapproved;
impl Evidence for Unapproved {
    fn approval(&self, _: &forge::Change) -> std::result::Result<Approval, String> {
        Err("no approval covers the head".into())
    }
    fn under_review(&self, _: &forge::Change) -> Option<String> {
        None
    }
    fn hold(&self, _: &forge::Change) -> Result<Option<String>> {
        Ok(None)
    }
    fn ci(&self, _: &str) -> Result<crate::forge::CommitChecks> {
        anyhow::bail!("not read")
    }
    fn required_checks(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn base(&self, change: &forge::Change) -> Result<(u64, bool)> {
        Ok((change.behind.unwrap(), change.overlap.unwrap()))
    }
    fn head(&self, change: &forge::Change) -> Result<String> {
        Ok(change.head.clone())
    }
}

fn mailbox(root: &Path) -> Vec<aida_core::mailbox::Message> {
    let Ok(entries) = std::fs::read_dir(root.join(".aida/mailbox")) else {
        return Vec::new();
    };
    let mut mail: Vec<aida_core::mailbox::Message> = entries
        .map(|e| serde_json::from_slice(&std::fs::read(e.unwrap().path()).unwrap()).unwrap())
        .collect();
    mail.sort_by(|a, b| a.subject.cmp(&b.subject));
    mail
}

fn agent(name: &str) -> aida_core::mailbox::Recipient {
    aida_core::mailbox::Recipient::Agent(name.into())
}

// (14)
#[test]
fn request_changes_mails_the_author_once_per_verdict() {
    let (temp, store) = fixture();
    let root = temp.path();
    // Round 1 at head 0xa, also archived: both files are named.
    write(root, "PR-7.json", &verdict(0xa, "request-changes"));
    write(root, "PR-7/0001.json", &verdict(0xa, "request-changes"));
    for _ in 0..3 {
        report(root, &store, &change(0xa), &Unapproved).unwrap();
    }
    let mail = mailbox(root);
    assert_eq!(mail.len(), 1, "one findings mail across passes");
    assert_eq!(mail[0].to, agent("Writer"), "the mapped author identity");
    for needle in [
        "`.aida/review-verdicts/PR-7.json`",
        "`.aida/review-verdicts/PR-7/0001.json`",
        "round 1 of 2",
        "2 finding(s)",
        "push a new head",
    ] {
        assert!(mail[0].body.contains(needle), "{needle}: {}", mail[0].body);
    }
    assert!(!mail[0].body.contains("--override"), "{}", mail[0].body);
    let entry = &store.load().unwrap().entries["PR-7"];
    assert!(entry.findings.values().all(|n| n.sent) && entry.cap.is_none());

    // A restarted watcher sends nothing more.
    let restarted = Store::open(root, REPO).unwrap();
    report(root, &restarted, &change(0xa), &Unapproved).unwrap();
    assert_eq!(mailbox(root).len(), 1);

    // A new blocking verdict at a new head mails again: the last round.
    write(root, "PR-7.json", &verdict(0xb, "request-changes"));
    report(root, &store, &change(0xb), &Unapproved).unwrap();
    report(root, &store, &change(0xb), &Unapproved).unwrap();
    let mail = mailbox(root);
    assert_eq!(mail.len(), 2, "{mail:#?}");
    let last = mail
        .iter()
        .find(|m| m.body.contains(&sha(0xb)))
        .expect("second findings mail");
    assert!(last.body.contains("round 2 of 2"), "{}", last.body);
    assert!(last.body.contains("last review round"), "{}", last.body);
    // Only the current head's receipt is kept.
    let entry = &store.load().unwrap().entries["PR-7"];
    assert_eq!(entry.findings.len(), 1);
    assert!(entry.findings.keys().all(|k| k.starts_with(&sha(0xb))));
    // The current head has a verdict: the cap wake is not due.
    assert!(entry.cap.is_none());
}

// (14) An approval never mails the author, even one following a blocking
// verdict on the same head.
#[test]
fn approval_never_mails_the_author() {
    let (temp, store) = fixture();
    let root = temp.path();
    write(root, "PR-7.json", &verdict(0xa, "approved"));
    report(root, &store, &change(0xa), &Unapproved).unwrap();
    let mut approved = verdict(0xa, "approved");
    approved["rounds"] = serde_json::json!([verdict(0xa, "request-changes")]);
    write(root, "PR-7.json", &approved);
    report(root, &store, &change(0xa), &Unapproved).unwrap();
    assert!(mailbox(root).is_empty(), "{:#?}", mailbox(root));
    assert!(store.load().unwrap().entries["PR-7"].findings.is_empty());
}

// (14) A failed delivery leaves the receipt pending, does not stop the
// readiness report, and is sent once on a later pass.
#[test]
fn failed_findings_delivery_is_retried_and_sent_once() {
    let (temp, store) = fixture();
    let root = temp.path();
    write(root, "PR-7.json", &verdict(0xa, "request-changes"));
    std::fs::write(root.join(".aida/mailbox"), "not a directory").unwrap();
    report(root, &store, &change(0xa), &Unapproved).unwrap();
    let entry = &store.load().unwrap().entries["PR-7"];
    assert!(entry.findings.values().all(|n| !n.sent));
    assert!(entry.readiness.is_some(), "the report still ran");
    std::fs::remove_file(root.join(".aida/mailbox")).unwrap();
    report(root, &store, &change(0xa), &Unapproved).unwrap();
    report(root, &store, &change(0xa), &Unapproved).unwrap();
    assert_eq!(mailbox(root).len(), 1);
    assert!(store.load().unwrap().entries["PR-7"]
        .findings
        .values()
        .all(|n| n.sent));
}

// (15)
#[test]
fn round_cap_wakes_the_advisor_once() {
    let (temp, store) = fixture();
    let root = temp.path();
    let mut second = verdict(0xb, "request-changes");
    second["rounds"] = serde_json::json!([verdict(0xa, "request-changes")]);
    write(root, "PR-7.json", &second);
    // A third head with no review: the watcher will not dispatch it.
    let c = change(0xc);
    assert_eq!(
        probe_unneeded(&store, &c).unwrap().as_deref(),
        Some("two-round review cap reached; route further work to the advisor")
    );
    for _ in 0..3 {
        report(root, &store, &c, &Unapproved).unwrap();
    }
    let mail = mailbox(root);
    assert_eq!(mail.len(), 1, "{mail:#?}");
    assert_eq!(mail[0].to, agent("advisor"));
    assert_eq!(
        mail[0].subject.as_deref(),
        Some("PR-7: two-round review cap reached")
    );
    for needle in [sha(0xa), sha(0xb), sha(0xc)] {
        assert!(mail[0].body.contains(&needle), "{needle}: {}", mail[0].body);
    }
    assert!(!mail[0].body.contains("--override"), "{}", mail[0].body);
    // Once per change: a later head, or a restarted watcher, wakes no one.
    let restarted = Store::open(root, REPO).unwrap();
    report(root, &restarted, &change(0xd), &Unapproved).unwrap();
    assert_eq!(mailbox(root).len(), 1);
    assert!(store.load().unwrap().entries["PR-7"]
        .cap
        .as_ref()
        .is_some_and(|n| n.sent));
}

/// A receipt key that is not `<head>:<digest>` makes the state unusable.
#[test]
fn malformed_findings_receipt_is_refused() {
    let (temp, store) = fixture();
    let root = temp.path();
    write(root, "PR-7.json", &verdict(0xa, "request-changes"));
    report(root, &store, &change(0xa), &Unapproved).unwrap();
    let path = root.join(".aida/review-watch").join(STATE);
    let text = std::fs::read_to_string(&path).unwrap();
    let key = store.load().unwrap().entries["PR-7"]
        .findings
        .keys()
        .next()
        .unwrap()
        .clone();
    std::fs::write(&path, text.replace(&key, &format!("{}:zz", sha(0xa)))).unwrap();
    let error = store.load().unwrap_err();
    assert!(
        format!("{error:#}").contains("invalid review-watch findings receipt"),
        "{error:#}"
    );
}

/// An approval carried over a clean base merge to the head; CI is never read,
/// so no ready wake joins the mailbox.
// trace:TASK-2238 | ai:claude
struct CarriedOver;
impl Evidence for CarriedOver {
    fn approval(&self, change: &forge::Change) -> std::result::Result<Approval, String> {
        Ok(Approval {
            reviewed: sha(0xb),
            identity: "operator at 2026-10-23".into(),
            carried: Some(format!("carried over clean merges of `{}`", change.base)),
        })
    }
    fn under_review(&self, change: &forge::Change) -> Option<String> {
        Unapproved.under_review(change)
    }
    fn hold(&self, change: &forge::Change) -> Result<Option<String>> {
        Unapproved.hold(change)
    }
    fn ci(&self, sha: &str) -> Result<crate::forge::CommitChecks> {
        Unapproved.ci(sha)
    }
    fn required_checks(&self) -> Result<Vec<String>> {
        Unapproved.required_checks()
    }
    fn base(&self, change: &forge::Change) -> Result<(u64, bool)> {
        Unapproved.base(change)
    }
    fn head(&self, change: &forge::Change) -> Result<String> {
        Unapproved.head(change)
    }
}

/// TASK-2242 (swept in TASK-2238): a capped change whose head is approved
/// through a carried-over approval does not wake the advisor with "its head
/// has no review".
// trace:TASK-2238 | ai:claude
#[test]
fn round_cap_wake_respects_a_carried_over_approval() {
    let (temp, store) = fixture();
    let root = temp.path();
    let mut second = verdict(0xb, "request-changes");
    second["rounds"] = serde_json::json!([verdict(0xa, "request-changes")]);
    write(root, "PR-7.json", &second);
    report(root, &store, &change(0xc), &CarriedOver).unwrap();
    assert!(mailbox(root).is_empty(), "{:#?}", mailbox(root));
    assert!(store
        .load()
        .unwrap()
        .entries
        .get("PR-7")
        .is_none_or(|e| e.cap.is_none()));
    // Without the approval the same change wakes the advisor.
    report(root, &store, &change(0xc), &Unapproved).unwrap();
    assert_eq!(mailbox(root).len(), 1);
}

/// TASK-2243 (swept in TASK-2238): a GitLab MR whose verdicts were read from
/// the historical `PR-N.json` alias names that file, not a missing
/// `MR-N.json`, in both the findings mail and the round-cap wake.
// trace:TASK-2238 | ai:claude
#[test]
fn gitlab_notices_name_the_pr_alias_file_they_were_read_from() {
    let mr = |head| forge::Change {
        gitlab: true,
        ..change(head)
    };
    let (temp, store) = fixture();
    let root = temp.path();
    write(root, "PR-7.json", &verdict(0xa, "request-changes"));
    report(root, &store, &mr(0xa), &Unapproved).unwrap();
    let mail = mailbox(root);
    assert_eq!(mail.len(), 1, "{mail:#?}");
    assert!(
        mail[0].body.contains("`.aida/review-verdicts/PR-7.json`"),
        "{}",
        mail[0].body
    );
    assert!(!mail[0].body.contains("MR-7.json"), "{}", mail[0].body);

    let (temp, store) = fixture();
    let root = temp.path();
    let mut second = verdict(0xb, "request-changes");
    second["rounds"] = serde_json::json!([verdict(0xa, "request-changes")]);
    write(root, "PR-7.json", &second);
    report(root, &store, &mr(0xc), &Unapproved).unwrap();
    let mail = mailbox(root);
    let cap = mail
        .iter()
        .find(|m| m.to == agent("advisor"))
        .expect("cap wake");
    assert!(
        cap.body.contains("`.aida/review-verdicts/PR-7.json`"),
        "{}",
        cap.body
    );
    assert!(!cap.body.contains("MR-7.json"), "{}", cap.body);
}
