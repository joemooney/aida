//! Read-only reconciliation evidence before recipient-UUID migration.
// trace:STORY-1843 | ai:codex
use aida_core::mailbox::{merge_dedup, message_state_rank, unread_inbox, Message, Recipient};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub(super) struct MigrationReport {
    report_version: u32,
    dry_run: bool,
    write_mode_available: bool,
    resolution_is_advisory: bool,
    sampled_at: String,
    source_fingerprint: String,
    source_files: usize,
    local_messages: usize,
    canonical_messages: usize,
    merged_messages: usize,
    broadcasts: usize,
    divergent_message_ids: Vec<String>,
    recipients: Vec<RecipientReport>,
    read_state: Vec<ReadState>,
}

#[derive(Debug, Serialize)]
struct RecipientReport {
    legacy_address: String,
    proposed_address: Option<String>,
    disposition: String,
    direct_messages: usize,
    legacy_unread_including_broadcasts: usize,
    diagnostics: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ReadState {
    legacy_identity_file: String,
    kind: String,
    receipt_file: Option<String>,
    candidate_message_ids: Vec<String>,
    diagnostics: Vec<String>,
    timestamp: i64,
}

// Record directory membership as well as opened-file digests. Refuse observed
// input changes; this is advisory evidence, not a transactional migration gate.
// This snapshot does not establish session liveness or authorize a future write.
// trace:STORY-1843 | ai:codex
#[derive(Default)]
struct Snapshot {
    files: BTreeMap<PathBuf, String>,
    dirs: BTreeMap<PathBuf, Vec<PathBuf>>,
}

impl Snapshot {
    fn directory(&mut self, path: &Path) -> Result<Vec<PathBuf>> {
        let entries = directory_entries(path)?;
        self.dirs.insert(path.to_path_buf(), entries.clone());
        Ok(entries)
    }

    fn read(&mut self, path: &Path) -> Result<Vec<u8>> {
        let bytes = read_regular(path)?;
        self.files
            .insert(path.to_path_buf(), format!("{:x}", Sha256::digest(&bytes)));
        Ok(bytes)
    }

    fn finish(&self) -> Result<String> {
        for (path, entries) in &self.dirs {
            if &directory_entries(path)? != entries {
                bail!(
                    "mailbox changed during preview: {}; retry --dry-run",
                    path.display()
                );
            }
        }
        let mut hash = Sha256::new();
        for (path, digest) in &self.files {
            let current = format!("{:x}", Sha256::digest(read_regular(path)?));
            if &current != digest {
                bail!(
                    "mailbox changed during preview: {}; retry --dry-run",
                    path.display()
                );
            }
            hash.update(path.as_os_str().as_encoded_bytes());
            hash.update([0]);
            hash.update(digest.as_bytes());
            hash.update([0]);
        }
        Ok(format!("{:x}", hash.finalize()))
    }
}

fn directory_entries(path: &Path) -> Result<Vec<PathBuf>> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("inspecting {}", path.display())),
        Ok(meta) if !meta.file_type().is_dir() => {
            bail!("preview requires an ordinary directory: {}", path.display())
        }
        Ok(_) => {}
    }
    let mut entries = std::fs::read_dir(path)
        .with_context(|| format!("reading {}", path.display()))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort();
    Ok(entries)
}

fn read_regular(path: &Path) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("opening preview input {}", path.display()))?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 || meta.len() > MAX_FILE_BYTES {
        bail!(
            "preview requires a singly linked regular file of at most 8 MiB: {}",
            path.display()
        );
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        bail!("preview input grew beyond 8 MiB: {}", path.display());
    }
    Ok(bytes)
}

fn messages(snapshot: &mut Snapshot, dir: &Path) -> Result<Vec<Message>> {
    let mut messages = Vec::new();
    let mut ids = BTreeSet::new();
    for path in snapshot.directory(dir)? {
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let message: Message = serde_json::from_slice(&snapshot.read(&path)?)
            .with_context(|| format!("invalid mailbox envelope {}", path.display()))?;
        if !ids.insert(message.id.clone()) {
            bail!("duplicate message ID '{}' in {}", message.id, dir.display());
        }
        messages.push(message);
    }
    Ok(messages)
}

fn timestamp(snapshot: &mut Snapshot, path: &Path) -> Result<i64> {
    let bytes = snapshot.read(path)?;
    std::str::from_utf8(&bytes)?
        .trim()
        .parse()
        .with_context(|| format!("invalid read-state timestamp {}", path.display()))
}

fn file_stem(path: &Path) -> Result<String> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .with_context(|| format!("invalid read-state filename {}", path.display()))
}

fn read_state(snapshot: &mut Snapshot, local: &Path) -> Result<Vec<ReadState>> {
    let mut state = Vec::new();
    for path in snapshot.directory(&local.join(".read"))? {
        let kind = match path.extension().and_then(|s| s.to_str()) {
            Some("txt") => "watermark",
            Some("seen") => "last-seen",
            _ => bail!("unsupported read-state entry {}", path.display()),
        };
        state.push(ReadState {
            legacy_identity_file: file_stem(&path)?,
            kind: kind.into(),
            receipt_file: None,
            candidate_message_ids: Vec::new(),
            diagnostics: Vec::new(),
            timestamp: timestamp(snapshot, &path)?,
        });
    }
    for identity in snapshot.directory(&local.join(".read-receipts"))? {
        let name = identity
            .file_name()
            .and_then(|s| s.to_str())
            .context("invalid receipt identity filename")?
            .to_string();
        for path in snapshot.directory(&identity)? {
            if path.extension().and_then(|s| s.to_str()) != Some("txt") {
                bail!("unsupported receipt entry {}", path.display());
            }
            state.push(ReadState {
                legacy_identity_file: name.clone(),
                kind: "receipt".into(),
                receipt_file: Some(
                    path.file_name()
                        .and_then(|s| s.to_str())
                        .context("invalid receipt filename")?
                        .to_string(),
                ),
                candidate_message_ids: Vec::new(),
                diagnostics: Vec::new(),
                timestamp: timestamp(snapshot, &path)?,
            });
        }
    }
    Ok(state)
}

// Preview only: no reservation, UUID assignment, acknowledgement, message copy,
// schema enablement, or canonical-store write is performed here.
// trace:STORY-1843 | ai:codex
pub(super) fn report(project: &Path, store: &Path) -> Result<MigrationReport> {
    let mut snapshot = Snapshot::default();
    let local_dir = crate::mailbox_store::mailbox_dir(project);
    let local = messages(&mut snapshot, &local_dir)?;
    let canonical = messages(&mut snapshot, &crate::mailbox_store::canonical_dir(store))?;
    let mut state = read_state(&mut snapshot, &local_dir)?;
    let merged = merge_dedup(&local, &canonical);
    for record in &mut state {
        if let Some(filename) = &record.receipt_file {
            let key = filename
                .strip_suffix(".txt")
                .context("unsupported receipt filename")?;
            record.candidate_message_ids = merged
                .iter()
                .filter(|m| crate::mailbox_store::sanitize_id(&m.id) == key)
                .map(|m| m.id.clone())
                .collect();
            record.candidate_message_ids.sort();
            record.diagnostics.push(match record.candidate_message_ids.len() {
                0 => "No current message matches this receipt filename; preserve unattributed metadata.",
                1 => "Filename match is advisory; historical sanitized IDs may have collided.",
                _ => "Receipt filename matches multiple message IDs; acknowledgement ownership is unresolved.",
            }.into());
        }
    }
    let directory = crate::mailbox_resolver::RecipientDirectory::load(project, store)?;
    let people = crate::known_mailbox_people(store);
    let marks: BTreeMap<_, _> = state
        .iter()
        .filter(|s| s.kind == "watermark")
        .map(|s| (s.legacy_identity_file.clone(), s.timestamp))
        .collect();
    let addresses: BTreeSet<_> = merged
        .iter()
        .filter_map(|m| match &m.to {
            Recipient::Agent(address) => Some(address.clone()),
            Recipient::Broadcast => None,
        })
        .collect();
    let mut marker_owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for address in &addresses {
        marker_owners
            .entry(crate::mailbox_store::sanitize_id(address))
            .or_default()
            .push(address.clone());
    }
    let mut recipients = Vec::new();
    for address in addresses {
        let mut diagnostics = Vec::new();
        let marker = crate::mailbox_store::sanitize_id(&address);
        if marker_owners[&marker].len() > 1 {
            diagnostics.push(format!("Legacy read-state filename collision: {:?}; acknowledgement ownership is unresolved.", marker_owners[&marker]));
        }
        let (proposed_address, disposition) = match directory.resolve(&address) {
            Ok(resolved) => {
                let disposition = if people.contains_key(&address.trim().to_lowercase()) {
                    "preserve-human"
                } else if let Some(note) = resolved.reader_notice {
                    diagnostics.push(note);
                    diagnostics.push("Historical shared group acknowledgements cannot establish private recipient receipts.".into());
                    "group-requires-explicit-cohort"
                } else {
                    "agent-requires-uuid-binding"
                };
                (Some(resolved.address), disposition)
            }
            Err(error) => {
                diagnostics.push(error.to_string());
                (None, "unresolved-preserve-history")
            }
        };
        recipients.push(RecipientReport {
            direct_messages: merged
                .iter()
                .filter(|m| matches!(&m.to, Recipient::Agent(to) if to == &address))
                .count(),
            legacy_unread_including_broadcasts: unread_inbox(
                &address,
                &merged,
                marks.get(&marker).copied(),
            )
            .len(),
            legacy_address: address,
            proposed_address,
            disposition: disposition.into(),
            diagnostics,
        });
    }
    let divergent_message_ids = local
        .iter()
        .filter(|l| canonical.iter().any(|c| c.id == l.id && c != *l))
        .map(|m| m.id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let fingerprint = snapshot.finish()?;
    Ok(MigrationReport {
        report_version: 1,
        dry_run: true,
        write_mode_available: false,
        resolution_is_advisory: true,
        sampled_at: chrono::Utc::now().to_rfc3339(),
        source_fingerprint: fingerprint,
        source_files: snapshot.files.len(),
        local_messages: local.len(),
        canonical_messages: canonical.len(),
        merged_messages: merged.len(),
        broadcasts: merged
            .iter()
            .filter(|m| matches!(m.to, Recipient::Broadcast) && message_state_rank(m) == 0)
            .count(),
        divergent_message_ids,
        recipients,
        read_state: state,
    })
}

pub(super) fn print_report(report: &MigrationReport, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        print!("{}", render_text(report)?);
    }
    Ok(())
}

fn render_text(report: &MigrationReport) -> Result<String> {
    use std::fmt::Write;
    let mut text = String::new();
    writeln!(
        text,
        "Mailbox migration preview - read-only; write mode unavailable"
    )?;
    writeln!(
        text,
        "{} local + {} canonical -> {} merged messages; {} live broadcasts",
        report.local_messages, report.canonical_messages, report.merged_messages, report.broadcasts
    )?;
    writeln!(
        text,
        "{} read-state records; mailbox snapshot {}",
        report.read_state.len(),
        report.source_fingerprint
    )?;
    writeln!(text, "{} message ID(s) have divergent local/canonical copies; the current view uses higher marker state, then canonical on ties.", report.divergent_message_ids.len())?;
    for id in &report.divergent_message_ids {
        writeln!(text, "  divergent ID: {id:?}")?;
    }
    for record in report
        .read_state
        .iter()
        .filter(|r| r.receipt_file.is_some() && r.candidate_message_ids.len() != 1)
    {
        writeln!(
            text,
            "  receipt needing reconciliation: {:?}/{:?}; candidate IDs {:?}",
            record.legacy_identity_file,
            record.receipt_file.as_deref().unwrap_or_default(),
            record.candidate_message_ids
        )?;
    }
    for recipient in &report.recipients {
        writeln!(
            text,
            "{:?}: {} direct, {} legacy unread · {} · proposed {:?}",
            recipient.legacy_address,
            recipient.direct_messages,
            recipient.legacy_unread_including_broadcasts,
            recipient.disposition,
            recipient
                .proposed_address
                .as_deref()
                .unwrap_or("unresolved")
        )?;
        for note in &recipient.diagnostics {
            writeln!(text, "  {}", note.escape_debug())?;
        }
    }
    writeln!(text, "Read-state keys retain original filenames. Receipt filename matches are advisory; identity rebinding and private acknowledgement ownership remain unresolved.")?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(id: &str, to: &str, timestamp: i64) -> Message {
        serde_json::from_value(json!({
            "id": id, "thread_id": "thread-1", "from": "sender",
            "to": {"kind":"agent", "agent":to},
            "timestamp": timestamp, "body": "private body is omitted from reports"
        }))
        .unwrap()
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn preview_preserves_legacy_watermarks_receipts_and_tombstones_without_writes() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        std::fs::create_dir_all(store.join("registry")).unwrap();
        std::fs::write(store.join("registry/team.toml"), "[members]\nJoe = []\n").unwrap();
        let local = crate::mailbox_store::mailbox_dir(project.path());
        std::fs::create_dir_all(local.join(".read")).unwrap();
        std::fs::create_dir_all(local.join(".read-receipts/Joe")).unwrap();
        std::fs::create_dir_all(store.join("mailbox")).unwrap();
        let first = message("seen", "Joe", 10);
        crate::mailbox_store::write_message(project.path(), &first).unwrap();
        crate::mailbox_store::write_message(project.path(), &message("unread", "Joe", 20)).unwrap();
        crate::mailbox_store::write_message(project.path(), &message("dead", "ended-worker", 30))
            .unwrap();
        let mut deleted = first.clone();
        deleted.deleted = true;
        std::fs::write(
            store.join("mailbox/seen.json"),
            serde_json::to_vec(&deleted).unwrap(),
        )
        .unwrap();
        std::fs::write(local.join(".read/Joe.txt"), "10").unwrap();
        std::fs::write(local.join(".read/Joe.seen"), "11").unwrap();
        std::fs::write(local.join(".read-receipts/Joe/seen.txt"), "11").unwrap();
        std::fs::write(local.join(".read/orphan.txt"), "99").unwrap();

        let first_report = report(project.path(), &store).unwrap();
        assert_eq!(first_report.merged_messages, 3);
        assert_eq!(first_report.divergent_message_ids, ["seen"]);
        let text = render_text(&first_report).unwrap();
        assert!(text.contains("1 message ID(s) have divergent"));
        assert!(text.contains("divergent ID: \"seen\""));
        assert_eq!(first_report.read_state.len(), 4);
        let human = first_report
            .recipients
            .iter()
            .find(|r| r.legacy_address == "Joe")
            .unwrap();
        assert_eq!(human.disposition, "preserve-human");
        assert_eq!(human.legacy_unread_including_broadcasts, 1);
        let dead = first_report
            .recipients
            .iter()
            .find(|r| r.legacy_address == "ended-worker")
            .unwrap();
        assert_eq!(dead.disposition, "unresolved-preserve-history");
        assert!(dead.proposed_address.is_none());
        let encoded = serde_json::to_string(&first_report).unwrap();
        assert!(!encoded.contains("private body is omitted"));
        assert!(!first_report.write_mode_available);
        let second_report = report(project.path(), &store).unwrap();
        assert_eq!(
            first_report.source_fingerprint,
            second_report.source_fingerprint
        );
        assert_eq!(
            std::fs::read_to_string(local.join(".read/Joe.txt")).unwrap(),
            "10"
        );
        assert_eq!(
            crate::mailbox_store::read_local_messages(project.path())
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            crate::mailbox_store::read_canonical_messages(&store).unwrap(),
            [deleted]
        );
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn preview_refuses_malformed_envelopes_and_read_state_instead_of_skipping_them() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        let local = crate::mailbox_store::mailbox_dir(project.path());
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("broken.json"), "{").unwrap();
        assert!(report(project.path(), &store)
            .unwrap_err()
            .to_string()
            .contains("invalid mailbox envelope"));
        std::fs::remove_file(local.join("broken.json")).unwrap();
        std::fs::create_dir_all(local.join(".read")).unwrap();
        std::fs::write(local.join(".read/worker.txt"), "unknown").unwrap();
        assert!(report(project.path(), &store)
            .unwrap_err()
            .to_string()
            .contains("invalid read-state timestamp"));
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn preview_refuses_links_and_observed_membership_changes() {
        let project = tempfile::tempdir().unwrap();
        let local = crate::mailbox_store::mailbox_dir(project.path());
        std::fs::create_dir_all(&local).unwrap();
        let target = project.path().join("message.json");
        std::fs::write(
            &target,
            serde_json::to_vec(&message("one", "Joe", 1)).unwrap(),
        )
        .unwrap();
        std::os::unix::fs::symlink(&target, local.join("linked.json")).unwrap();
        assert!(report(project.path(), &project.path().join(".aida-store")).is_err());
        std::fs::remove_file(local.join("linked.json")).unwrap();
        let mut snapshot = Snapshot::default();
        snapshot.directory(&local).unwrap();
        std::fs::write(local.join("new.json"), "{}").unwrap();
        assert!(snapshot
            .finish()
            .unwrap_err()
            .to_string()
            .contains("changed during preview"));
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn preview_reports_colliding_legacy_read_state_keys_without_guessing_an_owner() {
        let project = tempfile::tempdir().unwrap();
        let store = project.path().join(".aida-store");
        for (id, recipient) in [("one", "a#b"), ("two", "a_b")] {
            crate::mailbox_store::write_message(project.path(), &message(id, recipient, 10))
                .unwrap();
        }
        let preview = report(project.path(), &store).unwrap();
        assert_eq!(preview.recipients.len(), 2);
        for recipient in preview.recipients {
            assert!(recipient
                .diagnostics
                .iter()
                .any(|d| d.contains("filename collision")));
        }
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn receipt_filename_candidates_do_not_claim_original_message_ids() {
        let project = tempfile::tempdir().unwrap();
        let local = crate::mailbox_store::mailbox_dir(project.path());
        std::fs::create_dir_all(local.join(".read-receipts/Joe")).unwrap();
        for (filename, id) in [
            ("first.json", "id.with.dot"),
            ("second.json", "id#with#dot"),
            ("third.json", "id_with_dot"),
        ] {
            std::fs::write(
                local.join(filename),
                serde_json::to_vec(&message(id, "Joe", 10)).unwrap(),
            )
            .unwrap();
        }
        std::fs::write(local.join(".read-receipts/Joe/id_with_dot.txt"), "20").unwrap();
        std::fs::write(local.join(".read-receipts/Joe/missing.txt"), "20").unwrap();
        let preview = report(project.path(), &project.path().join(".aida-store")).unwrap();
        let collision = preview
            .read_state
            .iter()
            .find(|r| r.receipt_file.as_deref() == Some("id_with_dot.txt"))
            .unwrap();
        assert_eq!(
            collision.candidate_message_ids,
            ["id#with#dot", "id.with.dot", "id_with_dot"]
        );
        assert!(collision.diagnostics[0].contains("ownership is unresolved"));
        let missing = preview
            .read_state
            .iter()
            .find(|r| r.receipt_file.as_deref() == Some("missing.txt"))
            .unwrap();
        assert!(missing.candidate_message_ids.is_empty());
        let text = render_text(&preview).unwrap();
        assert!(text.contains("id_with_dot.txt"));
        assert!(text.contains("missing.txt"));
        assert!(!serde_json::to_value(&preview).unwrap()["read_state"][0]
            .as_object()
            .unwrap()
            .contains_key("message_id"));
    }

    // trace:STORY-1843 | ai:codex
    #[test]
    fn migration_requires_explicit_dry_run_and_remains_a_read_operation() {
        use crate::mailbox_cmd::{mailbox_cli_policy, MailboxArgs};
        use clap::{CommandFactory, Parser};
        assert!(MailboxArgs::try_parse_from(["mailbox", "migrate"]).is_err());
        let matches = MailboxArgs::command()
            .try_get_matches_from(["mailbox", "migrate", "--dry-run", "--json"])
            .unwrap();
        assert!(matches!(
            mailbox_cli_policy(&matches).stakeholder,
            crate::stakeholder_role::StakeholderAction::Read
        ));
    }
}
