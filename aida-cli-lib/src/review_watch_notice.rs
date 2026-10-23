//! Review notices, each delivered once: the change author's findings mail
//! for a blocking verdict on the current head, and the advisor's wake when a
//! change reaches the two-round cap with an unreviewed head. Report only:
//! nothing here starts a review, merges, pushes or updates a branch.
// trace:TASK-1945 | ai:claude
use super::*;

const DIGEST_LEN: usize = 16;

pub(super) fn valid_digest(digest: &str) -> bool {
    digest.len() == DIGEST_LEN && digest.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(super) fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text))[..DIGEST_LEN].to_string()
}

/// Record and deliver every notice `history` calls for. A receipt is marked
/// sent only after its delivery; a pending one keeps the message it was
/// recorded with, so a retry re-delivers it under the same ID.
///
/// The round-cap wake is for a head nobody approved: an approval that covers
/// the head through `evidence` (including one carried over a clean merge of
/// the base) suppresses it. That read runs only when the cap would fire.
// trace:TASK-2238 | ai:claude
pub(super) fn send(
    root: &Path,
    store: &Store,
    change: &forge::Change,
    history: &History,
    evidence: &dyn ready::Evidence,
) -> Result<()> {
    let key = change.key();
    let blocking = findings_due(change, history);
    let cap = history.blocking() >= 2
        && !history.contains(&change.head)
        && evidence.approval(change).is_err();
    if blocking.is_empty() && !cap {
        return Ok(());
    }
    let author = (!blocking.is_empty()).then(|| store.notice_recipient(&change.author));
    let pending = store.transaction(&key, |state| {
        let repository = state.repository.clone();
        let entry = state.entries.entry(key.clone()).or_default();
        // Findings for an earlier head are moot once the author pushed.
        let prefix = format!("{}:", change.head);
        entry.findings.retain(|k, _| k.starts_with(&prefix));
        let mut pending = Vec::new();
        for verdict in &blocking {
            let notice = entry
                .findings
                .entry(format!(
                    "{prefix}{}",
                    digest(&serde_json::to_string(verdict)?)
                ))
                .or_insert_with(|| Notice {
                    message: findings_message(
                        &repository,
                        change,
                        history,
                        verdict,
                        author.as_deref().unwrap_or("advisor"),
                    ),
                    sent: false,
                });
            if !notice.sent {
                pending.push(notice.clone());
            }
        }
        if cap {
            let notice = entry.cap.get_or_insert_with(|| Notice {
                message: cap_message(&repository, change, history),
                sent: false,
            });
            if !notice.sent {
                pending.push(notice.clone());
            }
        }
        Ok(pending)
    })?;
    for notice in pending {
        deliver(root, &notice)?;
        store.transaction(&key, |state| {
            let entry = state
                .entries
                .get_mut(&key)
                .context("review notice receipt disappeared")?;
            let receipt = entry
                .findings
                .values_mut()
                .chain(entry.cap.as_mut())
                .find(|n| n.message.id == notice.message.id)
                .context("review notice receipt disappeared")?;
            receipt.sent = true;
            Ok(())
        })?;
        eprintln!(
            "{key}: {} delivered to {}",
            notice.message.subject.as_deref().unwrap_or("review notice"),
            match &notice.message.to {
                aida_core::mailbox::Recipient::Agent(to) => to.as_str(),
                aida_core::mailbox::Recipient::Broadcast => "everyone",
            }
        );
    }
    Ok(())
}

/// The blocking verdicts on the current head, unless an approval of that
/// head followed: an approval never mails the author.
fn findings_due<'a>(
    change: &forge::Change,
    history: &'a History,
) -> Vec<&'a crate::review_verdict::RecordingKey> {
    let on_head = |key: &crate::review_verdict::RecordingKey| {
        key.0
            .as_deref()
            .is_some_and(|sha| crate::review_verdict::same_reviewed_sha(sha, &change.head))
    };
    let kind = |key: &crate::review_verdict::RecordingKey| {
        crate::review_verdict::VerdictKind::parse(&key.2)
    };
    if history
        .keys
        .iter()
        .any(|key| on_head(key) && kind(key).approves())
    {
        return Vec::new();
    }
    history
        .keys
        .iter()
        .filter(|key| on_head(key) && kind(key).blocks_done())
        .collect()
}

fn message(id: String, to: &str, subject: String, body: String) -> aida_core::mailbox::Message {
    use aida_core::mailbox::{Intent, Message, Recipient, SenderSource};
    Message {
        id: id.clone(),
        thread_id: id,
        from: "review-watch".into(),
        to: Recipient::Agent(to.to_string()),
        timestamp: chrono::Utc::now().timestamp(),
        in_reply_to: None,
        body,
        subject: Some(subject),
        urgent: false,
        intent: Intent::Request,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: SenderSource::Explicit,
        from_role: None,
        relayed_from: None,
    }
}

/// The author's findings mail. Its ID binds the change, head and verdict
/// recording, so a new blocking verdict is a new mail.
fn findings_message(
    repository: &str,
    change: &forge::Change,
    history: &History,
    verdict: &crate::review_verdict::RecordingKey,
    to: &str,
) -> aida_core::mailbox::Message {
    let key = change.key();
    let recording = serde_json::to_string(verdict).unwrap_or_default();
    let id = format!(
        "review-watch-{:x}",
        Sha256::digest(format!(
            "{repository}\0{key}\0{}\0findings\0{recording}",
            change.head
        ))
    );
    // The current verdict file always holds the latest round; an archived
    // round file is named when this verdict was also read from one.
    let mut files = vec![format!(
        ".aida/review-verdicts/{}",
        verdict_file(&key, history)
    )];
    for file in history.files.get(verdict).into_iter().flatten() {
        let path = format!(".aida/review-verdicts/{file}");
        if !files.contains(&path) {
            files.push(path);
        }
    }
    let files = files
        .iter()
        .map(|f| format!("`{f}`"))
        .collect::<Vec<_>>()
        .join(" and ");
    let round = history.blocking().clamp(1, RETAINED_ROUNDS);
    let next = if round >= RETAINED_ROUNDS {
        "This was the last review round under the two-round cap: a new head is not reviewed \
         automatically, and the advisor decides the next step."
            .to_string()
    } else {
        "Address them and push a new head; the watcher starts its review when it sees the head."
            .to_string()
    };
    let body = format!(
        "{key} at {head}: the review by {by} recorded `{word}` with {count} finding(s), \
         round {round} of {RETAINED_ROUNDS}. The findings are in {files}. {next}",
        head = change.head,
        by = if verdict.1.is_empty() {
            "the reviewer"
        } else {
            verdict.1.as_str()
        },
        word = verdict.2,
        count = verdict.3.len(),
    );
    message(
        id,
        to,
        format!("{key}: review requested changes (round {round} of {RETAINED_ROUNDS})"),
        body,
    )
}

/// The advisor's round-cap wake. Its ID binds the change and the blocking
/// rounds that reached the cap, so the change wakes the advisor once.
fn cap_message(
    repository: &str,
    change: &forge::Change,
    history: &History,
) -> aida_core::mailbox::Message {
    let key = change.key();
    let rounds: Vec<_> = history.blocking_keys().collect();
    let id = format!(
        "review-watch-{:x}",
        Sha256::digest(format!(
            "{repository}\0{key}\0cap\0{}",
            serde_json::to_string(&rounds).unwrap_or_default()
        ))
    );
    let shas = rounds
        .iter()
        .map(|r| format!("`{}`", r.0.as_deref().unwrap_or("unknown")))
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!(
        "{key} reached the two-round review cap: the reviews of {shas} each blocked done. \
         Its head {head} has no review, and the watcher will not start another. Decide the next \
         step: review the head by hand, or re-scope or close the change. Its findings are in \
         `.aida/review-verdicts/{file}`.",
        head = change.head,
        file = verdict_file(&key, history),
    );
    message(
        id,
        "advisor",
        format!("{key}: two-round review cap reached"),
        body,
    )
}

/// The current verdict file `history` was read from, relative to
/// `.aida/review-verdicts/`: `<key>.json`, or for a GitLab MR whose verdicts
/// sit in the historical `PR-N.json` alias, that alias.
// trace:TASK-2238 | ai:claude
fn verdict_file(key: &str, history: &History) -> String {
    let own = format!("{key}.json");
    let read: BTreeSet<&str> = history
        .files
        .values()
        .flatten()
        .map(String::as_str)
        .filter(|file| !file.contains('/'))
        .collect();
    if read.contains(own.as_str()) {
        return own;
    }
    key.strip_prefix("MR-")
        .map(|number| format!("PR-{number}.json"))
        .filter(|alias| read.contains(alias.as_str()))
        .unwrap_or(own)
}
