use super::*;
use aida_core::models::Requirement;
use aida_core::{CachedGitBackend, DatabaseBackend, RequirementStatus};
use tempfile::tempdir;

fn empty_user_ctx(role: Option<&str>) -> UserStatusContext {
    UserStatusContext {
        session: None,
        role: role.map(String::from),
        branch: None,
        pr: None,
        queue_head: Vec::new(),
        queue_total: 0,
        agents: Vec::new(),
    }
}

fn open_backend(project_root: &std::path::Path) -> CachedGitBackend {
    let store_root = project_root.join("store");
    let cache_path = project_root.join(".aida").join("cache.db");
    std::fs::create_dir_all(&store_root).unwrap();
    CachedGitBackend::open(&store_root, &cache_path).unwrap()
}

// STORY-465 acceptance: with no findings, no escalations, no briefs,
// and `--no-ci` short-circuiting the gh call, the report comes back
// empty so the section stays hidden. The quiet-day signal. trace:STORY-465
#[test]
fn empty_state_produces_hidden_report() {
    let dir = tempdir().unwrap();
    let backend = open_backend(dir.path());
    let ctx = empty_user_ctx(Some("implementer"));

    let report = collect_awaiting_report(dir.path(), &backend, &ctx, true);
    assert!(
        report.is_empty(),
        "fresh project must have nothing awaiting"
    );
    assert_eq!(report.total(), 0);
}

// STORY-741: unread mail folds into the awaiting-you report, and the
// `no_ci` path used by the per-turn notice makes NO network call — the
// gh-backed PR channel stays empty. A broadcast from another agent lands
// in every identity's inbox and, with no read-watermark, is unread.
// trace:STORY-741
#[test]
fn awaiting_report_folds_in_unread_mail_without_a_network_call() {
    let dir = tempdir().unwrap();
    let backend = open_backend(dir.path());

    let msg = aida_core::mailbox::Message {
        id: "m-awaiting-mail-1".to_string(),
        thread_id: "t1".to_string(),
        from: "other-agent".to_string(),
        to: aida_core::mailbox::Recipient::Broadcast,
        timestamp: 1_000,
        in_reply_to: None,
        body: "please review the drain".to_string(),
        urgent: true,
        intent: aida_core::mailbox::Intent::Fyi,
        retracted: false,
        deleted: false,
    };
    crate::mailbox_store::write_message(dir.path(), &msg).unwrap();

    let ctx = empty_user_ctx(Some("implementer"));
    // no_ci = true → the gh-backed PR probe is skipped (the network-free
    // path the per-turn notice rides).
    let report = collect_awaiting_report(dir.path(), &backend, &ctx, true);

    assert!(
        report.mail.unread >= 1,
        "unread mail must populate the mail channel"
    );
    assert_eq!(report.mail.urgent, 1, "urgent flag must carry through");
    assert!(
        report.mergeable_prs.is_empty(),
        "no_ci must skip the network PR probe"
    );
    // The report is now non-empty on mail alone, so the per-turn line fires.
    let line = report
        .compact_line()
        .expect("a report with unread mail yields a per-turn line");
    assert!(
        line.contains("mail"),
        "compact line must name the mail channel: {line}"
    );
}

// A spec parked in NeedsAttention surfaces as an escalation line —
// the implementer→advisor→human cascade landing in front of the
// operator without them having to grep `aida list --status`.
#[test]
fn needs_attention_spec_surfaces_as_escalation() {
    let dir = tempdir().unwrap();
    let backend = open_backend(dir.path());

    let mut parked = Requirement::new("punted overnight".into(), String::new());
    parked.spec_id = Some("SPIKE-12".into());
    parked.status = RequirementStatus::NeedsAttention;
    backend.add_requirement(parked).unwrap();

    let ctx = empty_user_ctx(Some("implementer"));
    let report = collect_awaiting_report(dir.path(), &backend, &ctx, true);

    assert!(!report.is_empty());
    assert_eq!(report.escalations.len(), 1);
    assert_eq!(report.escalations[0].spec_id, "SPIKE-12");
    assert_eq!(report.escalations[0].title, "punted overnight");
}

// Briefs are filed under `.aida/agent-briefs/<agent>/`. The classifier
// narrows to the running agent's directory when `AIDA_AGENT_TYPE` is a
// known value, so we set it explicitly to make detection deterministic
// under any test env. trace:STORY-465 | ai:claude
#[test]
fn unacked_briefs_for_running_agent_surface_in_the_report() {
    let dir = tempdir().unwrap();
    let backend = open_backend(dir.path());
    let briefs_dir = dir.path().join(".aida").join("agent-briefs").join("claude");
    std::fs::create_dir_all(&briefs_dir).unwrap();
    std::fs::write(
        briefs_dir.join("STORY-100.md"),
        "---\nspec_id: STORY-100\ngenerated_at: 2026-05-25T00:00:00Z\n---\nbody\n",
    )
    .unwrap();

    // Pin agent detection to `claude` so the classifier narrows to the
    // dir our fixture wrote to, regardless of the test host's env.
    let prior = std::env::var("AIDA_AGENT_TYPE").ok();
    // SAFETY: env mutation is bounded by this test; restored on exit.
    unsafe {
        std::env::set_var("AIDA_AGENT_TYPE", "claude");
    }

    let ctx = empty_user_ctx(Some("implementer"));
    let report = collect_awaiting_report(dir.path(), &backend, &ctx, true);

    assert_eq!(
        report.pending_briefs.len(),
        1,
        "the filed brief surfaces for the running agent"
    );
    assert_eq!(report.pending_briefs[0].spec_id, "STORY-100");
    assert_eq!(report.pending_briefs[0].agent, "claude");

    // SAFETY: same single-threaded reasoning — restore the env.
    unsafe {
        match prior {
            Some(v) => std::env::set_var("AIDA_AGENT_TYPE", v),
            None => std::env::remove_var("AIDA_AGENT_TYPE"),
        }
    }
}

// Reviewer-role queue items only surface when the active role IS
// `reviewer` — otherwise the Awaiting-you section would duplicate
// the Queue section directly below it. trace:STORY-465 | ai:claude
#[test]
fn reviewer_queue_items_only_surface_for_reviewer_role() {
    let dir = tempdir().unwrap();
    let backend = open_backend(dir.path());

    let queue_row = QueueRow {
        spec_id: "PR-42".into(),
        title: "verdict needed".into(),
        status: "Approved".into(),
        for_role: Some("reviewer".into()),
        in_progress: false,
        lease_id: None,
        lease_started_at: None,
    };

    // Implementer role: reviewer-queue items must NOT surface here.
    let mut ctx_impl = empty_user_ctx(Some("implementer"));
    ctx_impl.queue_head = vec![queue_row.clone()];
    let report = collect_awaiting_report(dir.path(), &backend, &ctx_impl, true);
    assert!(
        report.reviewer_queue_items.is_empty(),
        "implementer must not see verdict items"
    );

    // Reviewer role: the same row surfaces as a verdict-needed line.
    let mut ctx_rev = empty_user_ctx(Some("reviewer"));
    ctx_rev.queue_head = vec![queue_row];
    let report = collect_awaiting_report(dir.path(), &backend, &ctx_rev, true);
    assert_eq!(report.reviewer_queue_items.len(), 1);
    assert_eq!(report.reviewer_queue_items[0].spec_id, "PR-42");
}

// TASK-1146: a directive enqueued by the human-audit path (the STORY-768
// enqueue writer onto the local worker-directive file) folds into the
// awaiting-you report — so the request surfaces in the unified inbox the
// advisor polls, not only in the worker directives view. The `no_ci` path
// (the per-turn notice) picks it up too: the channel is a single local
// file read, no network. trace:TASK-1146
#[test]
fn enqueued_worker_directive_surfaces_in_awaiting_report() {
    let dir = tempdir().unwrap();
    let backend = open_backend(dir.path());

    // Fresh project: no directive file → the channel is empty and silent.
    let ctx = empty_user_ctx(Some("advisor"));
    let report = collect_awaiting_report(dir.path(), &backend, &ctx, true);
    assert_eq!(report.worker_directives.pending, 0);
    assert!(report.worker_directives.next.is_none());

    // Enqueue via the exact writer `aida human audit` uses.
    crate::human_audit::post_directive_line_enqueue(dir.path()).unwrap();

    let report = collect_awaiting_report(dir.path(), &backend, &ctx, true);
    assert_eq!(
        report.worker_directives.pending, 1,
        "the enqueued directive must populate the channel"
    );
    assert_eq!(
        report.worker_directives.next.as_deref(),
        Some(crate::human_audit::directive_line().as_str()),
        "the FIFO-head summary names the enqueued directive"
    );
    assert!(!report.is_empty(), "directives alone make the report fire");

    // The per-turn notice line names the channel (network-free path).
    let line = report
        .compact_line()
        .expect("a report with a pending directive yields a per-turn line");
    assert!(
        line.contains("1 directive"),
        "compact line must name the directives channel: {line}"
    );
}

// STORY-769: the `--notice` command ALWAYS leads with a time+timing line,
// even when nothing awaits (the deliberate silent-when-empty → one-line-
// every-turn contract change). The awaiting-channels half stays empty; the
// time line is unconditional. `format_notice_time_line` is the pure shape,
// and `stamp_turn_clock` always returns a non-empty label — so the emitted
// line is never blank regardless of the awaiting report. trace:STORY-769
#[test]
fn notice_time_line_is_always_emitted_and_well_formed() {
    // The awaiting report is empty (nothing awaits) — the old contract would
    // have printed nothing at all.
    let report = crate::awaiting_you::AwaitingReport::default();
    assert!(report.compact_line().is_none(), "nothing awaits");

    // The time line is emitted regardless, with the exact contract shape.
    let label = "first prompt of this session";
    let line = format_notice_time_line("Tuesday 2026-07-14 20:52 PDT", label);
    assert_eq!(
        line,
        "Current date/time: Tuesday 2026-07-14 20:52 PDT. Timing: first prompt of this session."
    );
    assert!(line.starts_with("Current date/time: "));
    assert!(line.contains(". Timing: "));

    // The label source is never empty for any event/prior combination — so
    // the emitted line can never degrade to a blank "Timing: .".
    let now = chrono::Utc::now();
    assert!(!presence::stamp_turn_clock(None, true, now).is_empty());
    assert!(!presence::stamp_turn_clock(None, false, now).is_empty());
}
