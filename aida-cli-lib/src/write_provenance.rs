//! Per-write provenance joined against the existing derived ledger, read-only.
// trace:STORY-1674 | ai:codex
use aida_core::{Comment, Requirement};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;

const USAGE_SCOPE: &str = "Cumulative session usage at or before each write; events overlap. Do not sum events as per-write or per-spec cost.";
const TOKEN_NAMES: [&str; 5] = [
    "input_uncached",
    "input_cache_write",
    "input_cache_read",
    "output",
    "output_reasoning",
];

#[derive(Debug, Serialize)]
struct TokenEvidence {
    /// Sum of known measurements; null if none were measured (distinct from zero).
    known: Option<u64>,
    unknown_records: u64,
}

#[derive(Debug, Serialize)]
struct SessionEvidence {
    measured_records: u64,
    models: BTreeSet<String>,
    unknown_model_records: u64,
    /// Ledger v1 has no explicit effort field. Never infer it from model/vendor.
    effort: Option<String>,
    tokens: std::collections::BTreeMap<&'static str, TokenEvidence>,
}

impl Default for SessionEvidence {
    fn default() -> Self {
        Self {
            measured_records: 0,
            models: BTreeSet::new(),
            unknown_model_records: 0,
            effort: None,
            tokens: TOKEN_NAMES
                .into_iter()
                .map(|name| {
                    (
                        name,
                        TokenEvidence {
                            known: None,
                            unknown_records: 0,
                        },
                    )
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct WriteEvent {
    kind: &'static str,
    id: String,
    timestamp: DateTime<Utc>,
    author: String,
    session_id: Option<String>,
    harness_version: Option<String>,
    session_usage: SessionEvidence,
}

#[derive(Debug, Serialize)]
struct ProvenanceView {
    spec_id: String,
    usage_scope: &'static str,
    ledger_available: bool,
    events: Vec<WriteEvent>,
}

fn present(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

fn exact_identity(value: Option<&str>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty()).map(str::to_owned)
}

fn session_evidence(
    conn: Option<&Connection>,
    session: Option<&str>,
    at: DateTime<Utc>,
) -> Result<SessionEvidence> {
    let mut evidence = SessionEvidence::default();
    let (Some(conn), Some(session)) = (conn, session) else {
        return Ok(evidence);
    };
    // Both persisted identity fields are exact matches, never prefixes. A LEFT
    // JOIN retains vendor-session evidence even without an attribution row.
    let mut stmt = conn.prepare("SELECT u.occurred_at,u.raw_model,u.input_uncached,u.input_cache_write,u.input_cache_read,u.output,u.output_reasoning FROM usage_record u LEFT JOIN attribution a USING(record_id) WHERE (u.vendor_session_id=?1 OR a.aida_session_id=?1) AND u.collection_status='measured' ORDER BY u.record_id")?;
    let mut rows = stmt.query([session])?;
    while let Some(row) = rows.next()? {
        let time: Option<String> = row.get(0)?;
        let Some(time) = time.and_then(|s| DateTime::parse_from_rfc3339(&s).ok()) else {
            continue;
        };
        if time.with_timezone(&Utc) > at {
            continue;
        }
        evidence.measured_records += 1;
        let model: Option<String> = row.get(1)?;
        if let Some(model) = present(model.as_deref()) {
            evidence.models.insert(model);
        } else {
            evidence.unknown_model_records += 1;
        }
        for (i, name) in TOKEN_NAMES.iter().enumerate() {
            let value: Option<i64> = row.get(i + 2)?;
            let item = evidence.tokens.get_mut(name).expect("fixed token names");
            match value {
                Some(value) => {
                    let value =
                        u64::try_from(value).context("negative token measurement in ledger")?;
                    item.known = Some(
                        item.known
                            .unwrap_or(0)
                            .checked_add(value)
                            .context("token measurement sum overflow")?,
                    );
                }
                None => item.unknown_records += 1,
            }
        }
    }
    Ok(evidence)
}

fn query(req: &Requirement, conn: Option<&Connection>) -> Result<ProvenanceView> {
    let mut events = vec![WriteEvent {
        kind: "filed",
        id: req.id.to_string(),
        timestamp: req.created_at,
        author: req.created_by.clone().unwrap_or_else(|| "unknown".into()),
        session_id: exact_identity(req.filed_at.as_ref().and_then(|p| p.session.as_deref())),
        harness_version: present(
            req.filed_at
                .as_ref()
                .and_then(|p| p.harness_version.as_deref()),
        ),
        session_usage: SessionEvidence::default(),
    }];
    for entry in &req.history {
        events.push(WriteEvent {
            kind: if entry.changes.is_empty() {
                "write"
            } else {
                "edit"
            },
            id: entry.id.to_string(),
            timestamp: entry.timestamp,
            author: entry.author.clone(),
            session_id: exact_identity(entry.session_id.as_deref()),
            harness_version: present(entry.harness_version.as_deref()),
            session_usage: SessionEvidence::default(),
        });
    }
    fn comments(events: &mut Vec<WriteEvent>, entries: &[Comment]) {
        for c in entries {
            events.push(WriteEvent {
                kind: if c.parent_id.is_some() {
                    "reply"
                } else {
                    "comment"
                },
                id: c.id.to_string(),
                timestamp: c.created_at,
                author: c.author.clone(),
                session_id: exact_identity(c.session_id.as_deref()),
                harness_version: present(c.harness_version.as_deref()),
                session_usage: SessionEvidence::default(),
            });
            comments(events, &c.replies);
        }
    }
    comments(&mut events, &req.comments);
    events.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then(a.id.cmp(&b.id)));
    for event in &mut events {
        event.session_usage = session_evidence(conn, event.session_id.as_deref(), event.timestamp)?;
    }
    Ok(ProvenanceView {
        spec_id: req.display_id().into(),
        usage_scope: USAGE_SCOPE,
        ledger_available: conn.is_some(),
        events,
    })
}

fn render(view: &ProvenanceView, json: bool, toon: bool) -> Result<String> {
    if json {
        return Ok(serde_json::to_string_pretty(view)?);
    }
    let mut out = if toon {
        format!(
            "spec_id: {}\nledger_available: {}\nusage_scope: {}\nevents:\n",
            view.spec_id, view.ledger_available, view.usage_scope
        )
    } else {
        format!(
            "Write provenance for {}\n{}\nLedger: {}\n",
            view.spec_id,
            view.usage_scope,
            if view.ledger_available {
                "available"
            } else {
                "missing; usage unknown"
            }
        )
    };
    // JSON quoted values keep authors and env-reported strings single-line.
    for e in &view.events {
        let text = |s: Option<&str>| serde_json::to_string(s.unwrap_or("unknown")).unwrap();
        let u = &e.session_usage;
        let models = if u.models.is_empty() {
            "unknown".into()
        } else {
            serde_json::to_string(&u.models)?
        };
        out.push_str(&format!("  {} {} {} author={} session={} harness_version={} models={} unknown_model_records={} effort=unknown measured_records={}\n", e.timestamp.to_rfc3339(), e.kind, e.id, text(Some(&e.author)), text(e.session_id.as_deref()), text(e.harness_version.as_deref()), models, u.unknown_model_records, u.measured_records));
        for (name, value) in &u.tokens {
            out.push_str(&format!(
                "    {name}: known={} unknown_records={}\n",
                value
                    .known
                    .map_or_else(|| "unknown".into(), |n| n.to_string()),
                value.unknown_records
            ));
        }
    }
    Ok(out)
}

pub(crate) fn show(root: &Path, req: &Requirement, json: bool, toon: bool) -> Result<()> {
    let path = root.join(".aida/derived/token-ledger-v1.sqlite");
    let conn = if path.try_exists()? {
        Some(
            Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .context("open existing token ledger read-only")?,
        )
    } else {
        None
    };
    println!("{}", render(&query(req, conn.as_ref())?, json, toon)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::models::{FilingProvenance, HistoryEntry};

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE usage_record(record_id TEXT PRIMARY KEY,vendor_session_id TEXT,occurred_at TEXT,raw_model TEXT,collection_status TEXT,input_uncached INTEGER,input_cache_write INTEGER,input_cache_read INTEGER,output INTEGER,output_reasoning INTEGER); CREATE TABLE attribution(record_id TEXT PRIMARY KEY,aida_session_id TEXT);
        INSERT INTO usage_record VALUES('one','vendor-session','2026-01-01T01:00:00+01:00','model-one','measured',0,NULL,2,3,NULL);
        INSERT INTO attribution VALUES('one','exact');
        INSERT INTO usage_record VALUES('two','exact','2025-12-31T17:00:01-07:00','model-two','measured',4,NULL,NULL,5,NULL);
        INSERT INTO usage_record VALUES('missing-model','exact','2026-01-01T00:00:02Z',NULL,'measured',NULL,NULL,NULL,0,NULL);
        INSERT INTO usage_record VALUES('future','exact','2026-01-01T00:00:04Z','future','measured',999,999,999,999,999);
        INSERT INTO usage_record VALUES('decoy','exact-other','2025-01-01T00:00:00Z','wrong-session','measured',999,999,999,999,999);
        INSERT INTO attribution VALUES('decoy','other');
        INSERT INTO usage_record VALUES('no-time','exact',NULL,'untimed','measured',999,999,999,999,999);
        INSERT INTO usage_record VALUES('bad-time','exact','invalid','bad-time','measured',999,999,999,999,999);
        INSERT INTO usage_record VALUES('unmeasured','exact','2025-01-01T00:00:00Z','unmeasured','unsupported',999,999,999,999,999);").unwrap();
        conn
    }

    fn at(time: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(time)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn story1674_exact_session_and_instant_cutoff_exclude_decoys_and_future_rows() {
        let conn = fixture();
        let early =
            session_evidence(Some(&conn), Some("exact"), at("2026-01-01T00:00:00Z")).unwrap();
        assert_eq!(early.measured_records, 1);
        assert_eq!(early.tokens["input_uncached"].known, Some(0));
        assert_eq!(early.tokens["output"].known, Some(3));
        let later =
            session_evidence(Some(&conn), Some("exact"), at("2026-01-01T00:00:03Z")).unwrap();
        assert_eq!(later.measured_records, 3);
        assert_eq!(
            later.models.into_iter().collect::<Vec<_>>(),
            vec!["model-one", "model-two"]
        );
        assert_eq!(later.unknown_model_records, 1);
        assert_eq!(later.tokens["input_uncached"].known, Some(4));
        assert_eq!(later.tokens["input_uncached"].unknown_records, 1);
        assert_eq!(later.tokens["output"].known, Some(8));
        assert_eq!(later.tokens["input_cache_write"].known, None);
        assert_eq!(later.tokens["input_cache_write"].unknown_records, 3);
        assert_eq!(later.effort, None);
        let prefix =
            session_evidence(Some(&conn), Some("exa"), at("2027-01-01T00:00:00Z")).unwrap();
        assert_eq!(prefix.measured_records, 0);
        assert_eq!(prefix.tokens["output"].known, None);
    }

    #[test]
    fn story1674_query_includes_nested_writes_and_keeps_legacy_unknown() {
        let conn = fixture();
        let mut req = Requirement::new("provenance".into(), "body".into());
        req.created_at = at("2026-01-01T00:00:00Z");
        req.filed_at = Some(FilingProvenance {
            session: Some("exact".into()),
            harness_version: Some("reported-only".into()),
            ..Default::default()
        });
        let mut history = HistoryEntry::new("historical".into(), Vec::new());
        history.timestamp = at("2026-01-01T00:00:01Z");
        req.history.push(history);
        let mut comment = Comment::new("writer\nwith newline".into(), "content excluded".into());
        comment.created_at = at("2026-01-01T00:00:02Z");
        comment.session_id = Some("exact".into());
        let mut reply = Comment::new_reply("writer".into(), "reply excluded".into(), comment.id);
        reply.created_at = at("2026-01-01T00:00:03Z");
        reply.session_id = Some("exact".into());
        comment.replies.push(reply);
        req.comments.push(comment);
        let view = query(&req, Some(&conn)).unwrap();
        assert_eq!(view.events.len(), 4);
        assert_eq!(view.events[0].session_usage.measured_records, 1);
        assert_eq!(view.events[1].session_usage.measured_records, 0);
        assert!(view.events[1].session_id.is_none());
        assert_eq!(view.events[3].kind, "reply");
        assert_eq!(view.events[3].session_usage.tokens["output"].known, Some(8));
        assert_eq!(
            view.events[0].harness_version.as_deref(),
            Some("reported-only")
        );
        for (json, toon) in [(true, false), (false, true), (false, false)] {
            let out = render(&view, json, toon).unwrap();
            assert!(out.contains("events") || out.contains("Write provenance"));
            assert!(out.contains("overlap"));
            assert!(!out.contains("content excluded"));
            assert!(!out.contains("reply excluded"));
            assert!(!out.contains("author=\"writer\n"));
        }
        let json: serde_json::Value =
            serde_json::from_str(&render(&view, true, false).unwrap()).unwrap();
        assert!(json["events"][1]["session_id"].is_null());
        assert!(json["events"][1]["session_usage"]["tokens"]["output"]["known"].is_null());
    }

    #[test]
    fn story1674_legacy_identity_is_not_trimmed_into_a_different_session() {
        let conn = fixture();
        let mut req = Requirement::new("legacy".into(), "body".into());
        req.created_at = at("2026-01-01T00:00:03Z");
        req.filed_at = Some(FilingProvenance {
            session: Some(" exact ".into()),
            ..Default::default()
        });
        let view = query(&req, Some(&conn)).unwrap();
        assert_eq!(view.events[0].session_id.as_deref(), Some(" exact "));
        assert_eq!(view.events[0].session_usage.measured_records, 0);
    }

    #[test]
    fn story1674_missing_ledger_and_unknown_session_do_not_borrow_usage() {
        let conn = fixture();
        for evidence in [
            session_evidence(None, Some("exact"), Utc::now()).unwrap(),
            session_evidence(Some(&conn), None, Utc::now()).unwrap(),
        ] {
            assert_eq!(evidence.measured_records, 0);
            assert!(evidence.models.is_empty());
            assert!(evidence.tokens.values().all(|t| t.known.is_none()));
        }
        let root = tempfile::tempdir().unwrap();
        show(
            root.path(),
            &Requirement::new("legacy".into(), "body".into()),
            true,
            false,
        )
        .unwrap();
        assert!(
            !root.path().join(".aida").exists(),
            "query must not create a ledger or derived directory"
        );
    }

    #[test]
    fn story1674_cli_comment_and_edit_persist_context() {
        use crate::cli::Cli;
        use aida_core::db::DatabaseBackend;
        use clap::Parser;
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_SESSION_ID", Some("cli-exact-session")),
            ("AIDA_HARNESS_VERSION", Some("cli-harness")),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("store");
        let backend = aida_core::GitBackend::new(&store_path).unwrap();
        let mut req = Requirement::new("CLI provenance".into(), "body".into());
        req.spec_id = Some("TASK-1".into());
        backend.add_requirement(req).unwrap();
        for args in [
            vec!["aida", "comment", "add", "TASK-1", "CLI note"],
            vec!["aida", "edit", "TASK-1", "--title", "CLI edit"],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            crate::git_backend_cmd::handle_git_backend_command(&store_path, &cli.command).unwrap();
        }
        let disk =
            aida_core::object_store::read_object(&store_path.join("objects"), "TASK-1").unwrap();
        assert_eq!(disk.title, "CLI edit");
        assert_eq!(
            disk.comments.last().unwrap().session_id.as_deref(),
            Some("cli-exact-session")
        );
        assert_eq!(
            disk.comments.last().unwrap().harness_version.as_deref(),
            Some("cli-harness")
        );
        assert_eq!(
            disk.history.last().unwrap().session_id.as_deref(),
            Some("cli-exact-session")
        );
        assert_eq!(
            disk.history.last().unwrap().harness_version.as_deref(),
            Some("cli-harness")
        );
    }

    #[test]
    fn story1674_cli_provenance_mode_and_existing_cost_mode_parse() {
        use crate::cli::{Cli, Command, UsageCommand};
        use clap::Parser;
        for args in [
            vec!["aida", "usage", "show", "TASK-1", "--provenance", "--json"],
            vec!["aida", "usage", "show", "TASK-1", "--provenance", "--toon"],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert!(matches!(
                cli.command,
                Command::Usage {
                    action: Some(UsageCommand::Show {
                        provenance: true,
                        cost: false,
                        ..
                    }),
                    ..
                }
            ));
        }
        let cli = Cli::try_parse_from(["aida", "usage", "show", "TASK-1", "--cost"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Usage {
                action: Some(UsageCommand::Show {
                    provenance: false,
                    cost: true,
                    ..
                }),
                ..
            }
        ));
        assert!(
            Cli::try_parse_from(["aida", "usage", "show", "TASK-1", "--cost", "--provenance"])
                .is_err()
        );
    }
}
