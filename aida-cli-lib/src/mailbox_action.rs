//! Explicit request dispositions; no read ACK, wake, or lifecycle mutation.
//! trace:TASK-3074 | ai:codex
use crate::mailbox_cmd::mailbox_cmd_args::DispositionOutcome;
use aida_core::mailbox::Message;
use aida_core::mailbox_identity::{IdentityEnv, MailIdentityFacts, PeopleCatalogView};
use aida_core::mailbox_lifecycle::action::{self, ActionPlan, ActionReceipt, Outcome};
use aida_core::mailbox_logical::{
    logical_snapshot, MailboxContext, Query, ReadLimits, ResolvedAudience,
};
use anyhow::{Context, Result};
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub(crate) fn record(
    project: &Path,
    store: &Path,
    id: &str,
    outcome: DispositionOutcome,
    reason: &str,
    evidence: Option<&str>,
    until: Option<&str>,
    json: bool,
) -> Result<()> {
    let until = until
        .map(chrono::DateTime::parse_from_rfc3339)
        .transpose()
        .context("--until must be an absolute RFC3339 timestamp")?
        .map(|t| t.timestamp_millis());
    let outcome = match outcome {
        DispositionOutcome::Acted => Outcome::Acted,
        DispositionOutcome::Declined => Outcome::Declined,
        DispositionOutcome::Deferred => Outcome::Deferred,
    };
    // Select the registry row before the leaf lock. The resolver below
    // only reads identity evidence: no Git, network, or additional locks.
    let row = crate::agent_handle::current_session_row_in(project);
    let resolve = || {
        let env = IdentityEnv::observed();
        let binding = crate::mailbox_root::session_binding_from_row(row.as_deref(), &env);
        let catalog = PeopleCatalogView::load(store);
        MailIdentityFacts::observed(binding, &catalog, env)
    };
    let facts = resolve();
    let audience = ResolvedAudience::from_facts(&facts);
    let context = MailboxContext::resolve(project, Some(store))
        .map_err(|e| anyhow::anyhow!("mail context refused: {e:?}"))?;
    let common = context
        .repository()
        .context("action receipts require a verified common Git directory")?
        .path();
    // Exact ID only, in the same logical snapshot as the inbox. No authority is
    // granted by sender access or an arbitrary --agent inspection selector.
    let snapshot = logical_snapshot(
        &context,
        &audience,
        &Query::ExactId(id.to_owned()),
        &ReadLimits::explicit_read(),
    );
    let record = snapshot
        .records
        .iter()
        .find(|r| r.message.id == id)
        .context("exact request ID not found in your authorized inbox")?;
    let receipt = ActionPlan::prepare(
        &snapshot, common, &audience, record, outcome, reason, evidence, until,
    )?
    .commit(|| {
        let current = resolve();
        if current != facts {
            anyhow::bail!("recipient identity evidence changed; disposition refused");
        }
        Ok(ResolvedAudience::from_facts(&current))
    })?;
    if json || crate::output_format_is_json() {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({"schema": 1, "action": receipt}))?
        );
    } else {
        println!("Disposition recorded: {}", serde_json::to_string(&receipt)?);
    }
    Ok(())
}

pub(crate) fn inspect(
    project: &Path,
    store: &Path,
    message: &Message,
) -> Result<Option<ActionReceipt>> {
    let context = match MailboxContext::resolve_or_local(project, Some(store)) {
        Ok(context) => context,
        // Existing source-local reads also work without a verifiable repository.
        // Such mail has no common-Git action evidence; its action stays unknown.
        Err(aida_core::mailbox_logical::SourceErrorCode::NotAGitCheckout) => return Ok(None),
        Err(e) => anyhow::bail!("mail context refused: {e:?}"),
    };
    match context.repository() {
        Some(repository) => action::latest(repository.path(), message),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mailbox_view::tests::{mail, mail_repo};

    #[test]
    fn disposition_cli_parses_contract_and_rejects_inspection_selector() {
        use crate::command_registry::try_parse_from;
        let valid = try_parse_from([
            "aida",
            "mailbox",
            "disposition",
            "id",
            "acted",
            "--reason",
            "handled",
            "--evidence",
            "PR-1",
            "--json",
        ]);
        assert!(valid.is_ok(), "{valid:?}");
        for args in [
            vec![
                "aida",
                "mailbox",
                "disposition",
                "id",
                "seen",
                "--reason",
                "read",
            ],
            vec!["aida", "mailbox", "disposition", "id", "acted"],
            vec![
                "aida",
                "mailbox",
                "disposition",
                "id",
                "declined",
                "--reason",
                "no",
                "--agent",
                "someone-else",
            ],
        ] {
            assert!(try_parse_from(args).is_err());
        }
    }

    #[test]
    fn disposition_adapter_writes_sibling_request_but_refuses_prefix_and_foreign_mail() {
        let _env = crate::test_env::AgentSessionIsolation::apply(&[
            ("AIDA_USER", Some("operator")),
            ("AIDA_SESSION_ROLE", Some("advisor")),
            ("AIDA_AGENT_TYPE", None),
        ]);
        let r = mail_repo();
        let request = Message {
            intent: aida_core::mailbox::Intent::Request,
            ..mail("sibling-request", "operator", 10, false)
        };
        crate::mailbox_store::write_message(&r.sib, &request).unwrap();
        for id in ["sibling", "missing"] {
            assert!(record(
                &r.main,
                &r.store,
                id,
                DispositionOutcome::Acted,
                "done",
                Some("PR-1"),
                None,
                true
            )
            .is_err());
        }
        assert!(record(
            &r.main,
            &r.store,
            &request.id,
            DispositionOutcome::Acted,
            "done",
            None,
            None,
            true
        )
        .is_err());
        assert!(record(
            &r.main,
            &r.store,
            &request.id,
            DispositionOutcome::Deferred,
            "later",
            None,
            Some("tomorrow"),
            true
        )
        .is_err());
        record(
            &r.main,
            &r.store,
            &request.id,
            DispositionOutcome::Acted,
            "done",
            Some("PR-1"),
            None,
            true,
        )
        .unwrap();
        let receipt = inspect(&r.sib, &r.store, &request).unwrap().unwrap();
        assert_eq!(receipt.actor, "operator");
        assert_eq!(receipt.evidence.as_deref(), Some("PR-1"));
        let view = crate::mailbox_view::read_own_mail(
            &r.main,
            &r.store,
            &crate::mailbox_view::own_audience_in(&r.main),
            &ReadLimits::explicit_read(),
        );
        assert_eq!(view.unread().len(), 1);
        let foreign = Message {
            id: "foreign-request".into(),
            to: aida_core::mailbox::Recipient::Agent("stranger".into()),
            ..request
        };
        crate::mailbox_store::write_message(&r.sib, &foreign).unwrap();
        assert!(record(
            &r.main,
            &r.store,
            &foreign.id,
            DispositionOutcome::Declined,
            "no",
            None,
            None,
            true
        )
        .is_err());
        assert!(inspect(&r.main, &r.store, &foreign).unwrap().is_none());
        let hinted = Message {
            id: "hinted-role-request".into(),
            to: aida_core::mailbox::Recipient::Agent("advisor".into()),
            ..foreign
        };
        crate::mailbox_store::write_message(&r.sib, &hinted).unwrap();
        let error = record(
            &r.main,
            &r.store,
            &hinted.id,
            DispositionOutcome::Declined,
            "no",
            None,
            None,
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("role hint"), "{error}");
        assert!(inspect(&r.main, &r.store, &hinted).unwrap().is_none());
    }

    #[test]
    fn verified_shared_reader_records_its_concrete_session_uuid() {
        use crate::mailbox_view::tests::{register_self_as_reviewer, D1_SESSION};
        let _env = crate::test_env::AgentSessionIsolation::apply(&[
            ("AIDA_USER", Some("operator")),
            ("AIDA_SESSION_ROLE", Some("reviewer")),
            ("AIDA_AGENT_TYPE", Some("codex")),
            ("CODEX_SESSION_ID", Some(D1_SESSION)),
        ]);
        let r = mail_repo();
        let audience = register_self_as_reviewer(&r.main);
        let request = Message {
            intent: aida_core::mailbox::Intent::Request,
            ..mail("shared-request", "reviewer", 10, false)
        };
        crate::mailbox_store::write_message(&r.sib, &request).unwrap();
        record(
            &r.main,
            &r.store,
            &request.id,
            DispositionOutcome::Acted,
            "review started",
            Some("PR-123"),
            None,
            true,
        )
        .unwrap();
        let receipt = inspect(&r.main, &r.store, &request).unwrap().unwrap();
        assert_eq!(receipt.recipient_uuid.as_ref(), audience.recipient());
        assert_eq!(&receipt.actor, &audience.private_keys()[0]);
        assert_eq!(receipt.keys, vec!["reviewer"]);
    }
}
