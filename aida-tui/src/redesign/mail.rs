//! Mailbox content source + send action for the redesign cockpit (STORY-701).
//!
//! EPIC-53 owns the CONTENT layer — what surfaces, how it's classified, what
//! actions it offers — while EPIC-54 (this crate's `redesign` shell) owns the
//! interaction gesture (see `docs/plans/2026-06-26-epic-53-cockpit-seam.md`).
//! This module is that content layer for mail: it projects unread inbox
//! messages into [`TargetItem`] rows (mirroring [`super::store::summary_to_item`]
//! for the Queue scope's own data source) and builds the argv for the "send"
//! action that wraps `aida mailbox send`.
//!
//! Every function below the IO-boundary line is PURE (no filesystem, no env)
//! and unit-tested without a terminal or a real `.aida/mailbox/` directory —
//! the STORY-701 acceptance criterion.
//!
//! trace:STORY-701 | ai:claude

use std::path::{Path, PathBuf};

use aida_core::mailbox::Message;

use super::state::TargetItem;

// ---------------------------------------------------------------------------
// Pure projection: Message -> TargetItem
// ---------------------------------------------------------------------------

/// Project one unread message into a cockpit row. Pure: no I/O, so it's
/// testable straight from `Message` fixtures. `id` is the message id (NOT a
/// spec id) and `req_type` is the fixed marker `"Mail"` the Mail scope's
/// row source is the only producer of, so a mail row is never mistaken for a
/// spec row by anything that inspects `req_type`.
// trace:STORY-701 | ai:claude
pub fn mail_item(m: &Message) -> TargetItem {
    TargetItem {
        id: m.id.clone(),
        title: mail_subject(&m.body, 60),
        req_type: "Mail".to_string(),
        status: mail_status(m),
        priority: if m.urgent {
            "urgent".to_string()
        } else {
            String::new()
        },
        body: m.body.clone(),
        has_test_plan: false,
        routed_role: None,
        tags: Vec::new(),
    }
}

/// Project a slice of unread messages into cockpit rows. Pure.
// trace:STORY-701 | ai:claude
pub fn mail_items(unread: &[&Message]) -> Vec<TargetItem> {
    unread.iter().map(|m| mail_item(m)).collect()
}

/// First non-empty line of a message body, trimmed and truncated to `max`
/// chars (with an ellipsis when cut) — the row's display title. Mirrors
/// `aida_core::mailbox`'s private notice-subject projection (kept
/// independent since that one isn't `pub`).
// trace:STORY-701 | ai:claude
fn mail_subject(body: &str, max: usize) -> String {
    let first = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if first.is_empty() {
        return "(empty message)".to_string();
    }
    let mut chars = first.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// The row's status column: sender, plus an urgent/actionable-intent flag —
/// the same signal `aida mailbox inbox` surfaces on its message lines.
// trace:STORY-701 | ai:claude
fn mail_status(m: &Message) -> String {
    let mut flags = Vec::new();
    if m.urgent {
        flags.push("urgent");
    }
    if m.intent.is_actionable() {
        flags.push(m.intent.as_str());
    }
    if flags.is_empty() {
        format!("from {}", m.from)
    } else {
        format!("from {} · {}", m.from, flags.join(", "))
    }
}

/// The sender of a mail row, resolved from the `TargetItem`'s `status`
/// column ("from <sender>[ · flags]") — the reply target for [`Verb::Reply`].
/// Kept as a small pure parser (rather than a dedicated `TargetItem` field)
/// so the Mail scope's reply gesture doesn't need its own struct field on the
/// otherwise spec-shaped `TargetItem`. `None` if the row wasn't produced by
/// [`mail_item`] (defensive; the Mail scope is the only caller).
// trace:STORY-701 | ai:claude
pub fn mail_sender(item: &TargetItem) -> Option<&str> {
    item.status
        .strip_prefix("from ")
        .map(|rest| rest.split(" · ").next().unwrap_or(rest))
}

// ---------------------------------------------------------------------------
// The "send" ACTION (STORY-701): pure argv builder wrapping `aida mailbox send`.
// ---------------------------------------------------------------------------

/// Build the argv for `aida mailbox send`, returning the exact argument list
/// (NOT a shell string) so a caller spawns it directly via
/// `Command::new(exe).args(...)` without threading an arbitrary message body
/// through shell quoting. Pure: no I/O, fully unit-testable — the send
/// action's registerable half of the seam.
// trace:STORY-701 | ai:claude
pub fn send_mail_argv(
    to: &str,
    body: &str,
    in_reply_to: Option<&str>,
    urgent: bool,
) -> Vec<String> {
    let mut argv = vec![
        "mailbox".to_string(),
        "send".to_string(),
        "--to".to_string(),
        to.to_string(),
    ];
    if let Some(id) = in_reply_to {
        argv.push("--in-reply-to".to_string());
        argv.push(id.to_string());
    }
    if urgent {
        argv.push("--urgent".to_string());
    }
    argv.push(body.to_string());
    argv
}

// ---------------------------------------------------------------------------
// IO boundary: reading the local mailbox layer + resolving this shell's
// identity / project root. Not unit-tested against a terminal — these are
// thin, best-effort wrappers around aida_core::mailbox + std::fs.
// ---------------------------------------------------------------------------

/// This shell's mail identity: the same precedence the CLI's queue/mailbox
/// user resolution uses (BUG-89) — `AIDA_USER`, then `USER`, then `USERNAME`
/// (Windows), then `"default"`.
// trace:STORY-701 | ai:claude
fn mail_identity() -> String {
    std::env::var("AIDA_USER")
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "default".to_string())
}

/// Resolve the AIDA project root from `cwd`.
///
/// TASK-1141: this MUST agree with the CLI's `aida mailbox send` (the reply
/// action shells out to it), or the cockpit can read the inbox from one root
/// while replies land in another's `.aida/mailbox/`. So this routes through
/// the exact same canonical resolver as the CLI —
/// `aida_core::store_locate::resolve_project_root_from`, which finds the
/// attached distributed store (honoring `AIDA_STORE`, and BUG-331's
/// main-worktree fallback for a linked/nested git worktree whose own
/// `.aida-store/` isn't attached) and takes its parent. Only when that
/// yields nothing (not a distributed AIDA project here — legacy/no store)
/// do we fall back to the old best-effort `.git`/`.aida/config.toml`
/// walk-up, matching [`fetch_mail_items`]'s own "any failure yields an
/// empty Vec" grace.
// trace:STORY-701 trace:TASK-1141 | ai:claude
fn resolve_project_root(cwd: &Path) -> PathBuf {
    if let Some(root) = aida_core::store_locate::resolve_project_root_from(cwd) {
        return root;
    }
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        if d.join(".git").exists() || d.join(".aida").join("config.toml").is_file() {
            return d.to_path_buf();
        }
        dir = d.parent();
    }
    cwd.to_path_buf()
}

/// Read this operator's unread mail from the LOCAL mailbox layer (the fast,
/// live-exchange layer) and project it into cockpit rows — the Mail scope's
/// item set. Read-only: never advances the watermark, so painting the
/// cockpit never marks mail seen (mirrors `aida mailbox inbox --peek`). Best
/// effort: any read failure yields an empty Vec so the cockpit still paints.
// trace:STORY-701 | ai:claude
pub fn fetch_mail_items() -> Vec<TargetItem> {
    let Ok(cwd) = std::env::current_dir() else {
        return Vec::new();
    };
    let project_root = resolve_project_root(&cwd);
    let messages = aida_core::mailbox::read_local_messages(&project_root).unwrap_or_default();
    let agent = mail_identity();
    let watermark = aida_core::mailbox::read_local_watermark(&project_root, &agent);
    let unread = aida_core::mailbox::unread_inbox(&agent, &messages, watermark);
    mail_items(&unread)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::mailbox::{Intent, Recipient};

    fn mail_msg(id: &str, from: &str, body: &str, urgent: bool, ts: i64) -> Message {
        Message {
            id: id.to_string(),
            thread_id: id.to_string(),
            from: from.to_string(),
            to: Recipient::Agent("you".to_string()),
            timestamp: ts,
            in_reply_to: None,
            body: body.to_string(),
            urgent,
            intent: Intent::Fyi,
            retracted: false,
            deleted: false,
        }
    }

    #[test]
    fn mail_items_projects_unread_messages() {
        let m1 = mail_msg("m1", "codex", "PR ready for review\nsecond line", false, 10);
        let m2 = mail_msg("m2", "agy", "quick heads up", true, 20);
        let unread: Vec<&Message> = vec![&m1, &m2];
        let rows = mail_items(&unread);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "m1");
        assert_eq!(rows[0].title, "PR ready for review");
        assert_eq!(rows[0].req_type, "Mail");
        assert_eq!(rows[0].status, "from codex");
        assert_eq!(rows[0].priority, "");
        assert_eq!(rows[1].status, "from agy · urgent");
        assert_eq!(rows[1].priority, "urgent");
    }

    #[test]
    fn mail_items_flags_actionable_intent() {
        let mut m = mail_msg("m1", "codex", "please review", false, 10);
        m.intent = Intent::Request;
        let unread = vec![&m];
        let rows = mail_items(&unread);
        assert_eq!(rows[0].status, "from codex · request");
    }

    #[test]
    fn mail_items_urgent_and_actionable_both_flag() {
        let mut m = mail_msg("m1", "codex", "drop everything", true, 10);
        m.intent = Intent::Handoff;
        let unread = vec![&m];
        let rows = mail_items(&unread);
        assert_eq!(rows[0].status, "from codex · urgent, handoff");
    }

    #[test]
    fn mail_items_truncates_long_subject_and_handles_empty_body() {
        let long_body = "x".repeat(80);
        let m1 = mail_msg("m1", "codex", &long_body, false, 10);
        let m2 = mail_msg("m2", "codex", "   \n  ", false, 20); // whitespace-only body
        let unread = vec![&m1, &m2];
        let rows = mail_items(&unread);
        assert!(rows[0].title.ends_with('…'));
        assert_eq!(rows[0].title.chars().count(), 61); // 60 chars + ellipsis
        assert_eq!(rows[1].title, "(empty message)");
    }

    #[test]
    fn mail_items_empty_input_is_empty() {
        assert!(mail_items(&[]).is_empty());
    }

    #[test]
    fn mail_sender_parses_the_status_column() {
        let m = mail_msg("m1", "codex", "hi", false, 10);
        let row = mail_item(&m);
        assert_eq!(mail_sender(&row), Some("codex"));
    }

    #[test]
    fn mail_sender_parses_past_trailing_flags() {
        let mut m = mail_msg("m1", "codex", "hi", true, 10);
        m.intent = Intent::Request;
        let row = mail_item(&m);
        assert_eq!(row.status, "from codex · urgent, request");
        assert_eq!(mail_sender(&row), Some("codex"));
    }

    #[test]
    fn mail_sender_none_for_a_non_mail_row() {
        let row = TargetItem {
            id: "TASK-1".to_string(),
            title: "not mail".to_string(),
            req_type: "Task".to_string(),
            status: "Draft".to_string(),
            priority: String::new(),
            body: String::new(),
            has_test_plan: false,
            routed_role: None,
            tags: Vec::new(),
        };
        assert_eq!(mail_sender(&row), None);
    }

    #[test]
    fn send_mail_argv_builds_minimal_command() {
        let argv = send_mail_argv("codex", "hello there", None, false);
        assert_eq!(
            argv,
            vec!["mailbox", "send", "--to", "codex", "hello there"]
        );
    }

    #[test]
    fn send_mail_argv_includes_reply_and_urgent_flags() {
        let argv = send_mail_argv("codex", "on it", Some("m1"), true);
        assert_eq!(
            argv,
            vec![
                "mailbox",
                "send",
                "--to",
                "codex",
                "--in-reply-to",
                "m1",
                "--urgent",
                "on it",
            ]
        );
    }

    #[test]
    fn resolve_project_root_finds_nearest_git_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(resolve_project_root(&nested), root);
    }

    #[test]
    fn resolve_project_root_finds_aida_config_when_no_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(root.join(".aida").join("config.toml"), "").unwrap();
        let nested = root.join("x");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(resolve_project_root(&nested), root);
    }

    // TASK-1141: the divergent case. Before this fix, `resolve_project_root`
    // walked up from cwd for the nearest `.git` (file OR dir) — and a
    // LINKED git worktree (e.g. under `.claude/worktrees/<name>`, as this
    // very repo uses) has its OWN `.git` file, so the old walk stopped
    // there and returned the linked worktree's root. Meanwhile the CLI's
    // `aida mailbox send` (which the reply action shells out to) resolves
    // the project root via the orphan-store worktree's parent — and since
    // `.aida-store/` is only attached in the MAIN worktree, that resolves
    // to the MAIN worktree's root instead. Read (TUI) and send (CLI) would
    // silently disagree.
    //
    // This test proves the fix: `resolve_project_root` on a linked worktree
    // now returns the SAME root the CLI's canonical resolver
    // (`aida_core::store_locate`) does — the MAIN worktree, not the linked
    // one — by routing through that shared resolver instead of the old
    // ad-hoc walk.
    // trace:TASK-1141 | ai:claude
    #[test]
    fn resolve_project_root_agrees_with_cli_in_nested_worktree() {
        fn git(repo: &std::path::Path, args: &[&str]) {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .output()
                .expect("git on PATH");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }

        let tmp = tempfile::tempdir().unwrap();
        let main_root = tmp.path().join("main");
        std::fs::create_dir_all(&main_root).unwrap();
        git(&main_root, &["init", "-q", "-b", "main"]);
        git(&main_root, &["config", "user.email", "t@t.t"]);
        git(&main_root, &["config", "user.name", "t"]);
        std::fs::create_dir_all(main_root.join(".aida")).unwrap();
        std::fs::write(
            main_root.join(".aida/config.toml"),
            "[deployment]\nmode = \"distributed\"\nstore_path = \".aida-store\"\n",
        )
        .unwrap();
        git(&main_root, &["add", "."]);
        git(&main_root, &["commit", "-q", "-m", "init"]);
        // The attached orphan-store worktree lives ONLY in the main worktree
        // — exactly the case that used to make the CLI and the TUI disagree.
        std::fs::create_dir_all(main_root.join(".aida-store")).unwrap();

        // A NESTED linked worktree, mirroring this repo's own
        // `.claude/worktrees/<agent>` convention.
        let nested = main_root.join(".claude/worktrees/agent-x");
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        git(
            &main_root,
            &["worktree", "add", "--detach", nested.to_str().unwrap()],
        );
        // Sanity: the nested worktree really does have its own `.git` (a
        // file, not a dir) — the thing the OLD walk-up would have stopped
        // at, returning `nested` instead of `main_root`.
        assert!(nested.join(".git").is_file());
        assert!(!nested.join(".aida-store").exists());

        let resolved = resolve_project_root(&nested);
        assert_eq!(
            resolved.canonicalize().unwrap(),
            main_root.canonicalize().unwrap(),
            "TUI mail path must resolve to the MAIN worktree root, matching \
             `aida mailbox send`, not the nested worktree it was invoked from"
        );
    }
}
