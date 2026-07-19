//! `aida statusline` command cluster (FR-1-041 / TASK-0414 / TASK-306 /
//! TASK-60 / TASK-282 / TASK-896 / STORY-78 / STORY-79).
//!
//! The interactive statusline surface: `handle_statusline_command` renders the
//! POSIX-safe one-liner (role / branch / queue-depth / freshness / session
//! anchor segments) that Claude Code and the shell `PROMPT_COMMAND` invoke, and
//! `handle_statusline_setup_command` drives the opt-in `aida statusline setup`
//! bootstrap (prints or installs client-appropriate statusline config). The
//! private helpers assemble the individual segments and the setup snippets.
//! Extracted verbatim from `main.rs` (SPIKE-78); no behavior change.

use anyhow::{Context, Result};
use colored::Colorize;

use crate::*;

pub(crate) fn handle_statusline_command(color: &str, title: bool) -> Result<()> {
    // trace:FR-1-041 | ai:claude
    // trace:TASK-896 — an OSC terminal-title string carries no ANSI, so
    // `--title` forces color off no matter what `--color` requested.
    apply_color_mode(if title { "never" } else { color });

    let project_root = statusline_project_root();
    let project_name = project_root
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "?".into());

    // Try to read project name from store metadata (fall back to dir name).
    let store_name = std::fs::read_to_string(project_root.join("aida-store/metadata.yaml"))
        .or_else(|_| std::fs::read_to_string(project_root.join(".aida-store/metadata.yaml")))
        .ok()
        .and_then(|content| {
            content
                .lines()
                .find_map(|l| {
                    l.strip_prefix("name:")
                        .map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string())
                })
                .filter(|s| !s.is_empty())
        });
    let project_label = store_name.unwrap_or(project_name);

    // TASK-645: resolve through the one role resolver. An unset/blank
    // env var becomes the implementer default (marked below) so the
    // statusline never shows a roleless `(none)`/blank segment.
    let raw_role = std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let (effective_role_name, role_is_default) = resolve_effective_role(raw_role.as_deref());

    // STORY-79: opportunistic background fetch. Fire-and-forget; uses
    // current state for this render, refreshed state next render.
    // trace:STORY-79 | ai:claude
    {
        let store_path = if project_root.join(".aida-store").exists() {
            project_root.join(".aida-store")
        } else {
            project_root.join("aida-store")
        };
        if store_path.exists() {
            maybe_spawn_bg_fetch(&project_root, &store_path);
        }
    }

    // Cache freshness state. Folds two axes (cache vs local, local vs
    // origin) into one severity-ordered label. None when there's no
    // cache.db yet (fresh `aida init` with no reads). Render contract
    // below: skip `Fresh`, surface everything else.
    // trace:STORY-78 | ai:claude
    let cache_path = project_root.join(".aida/cache.db");
    let cache_freshness = if cache_path.exists() {
        match rusqlite::Connection::open_with_flags(
            &cache_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(conn) => {
                let recorded_sha: Option<String> = conn
                    .query_row(
                        "SELECT value FROM cache_meta WHERE key = 'source_head_sha'",
                        [],
                        |r| r.get(0),
                    )
                    .ok();
                let local_sha = aida_core::git_ops::head_sha(&project_root.join(".aida-store"))
                    .ok()
                    .or_else(|| aida_core::git_ops::head_sha(&project_root.join("aida-store")).ok())
                    .unwrap_or_default();
                let origin_sha = rev_parse_origin_aida_store(&project_root);
                let last_fetch_age = read_last_fetch_age_secs(&project_root);
                // Resolve direction (local behind / equal / ahead /
                // unknown) before passing to the pure classifier, so
                // unpushed-local-commits don't false-positive as
                // "behind". trace:STORY-78 | ai:claude
                let store_path = if project_root.join(".aida-store").exists() {
                    project_root.join(".aida-store")
                } else {
                    project_root.join("aida-store")
                };
                let local_behind = match (&local_sha, origin_sha.as_deref()) {
                    (l, Some(o)) if !l.is_empty() => local_lags_origin(&store_path, l, o),
                    _ => None,
                };
                Some(classify_cache_freshness(
                    recorded_sha.as_deref(),
                    &local_sha,
                    local_behind,
                    last_fetch_age,
                    cache_freshness_threshold_secs(),
                ))
            }
            Err(_) => None,
        }
    } else {
        None
    };

    // Queue depth — count entries in the user's queue routed to the
    // active role (or all entries when no role is set). Reads the orphan
    // store's queues/<user>.yaml file directly to keep this off the
    // backend's heavier load() path.
    // TASK-645: depth for the effective role (implementer when unset), so
    // the default sees its own queue rather than the all-entries view.
    let queue_depth =
        read_queue_depth(&project_root, Some(effective_role_name.as_str())).unwrap_or(0);

    let separator = " · ".dimmed().to_string();
    // Brand anchor: Greek transliteration of "AIDA". Same 4-column
    // footprint as the prior literal "aida"; the δ is the only
    // unmistakably-Greek glyph, so the prefix carries a quiet identity
    // marker without looking foreign. trace:TASK-1-048 | ai:claude
    let mut parts: Vec<String> = vec![
        "αιδα".dimmed().to_string(),
        project_label.green().bold().to_string(),
    ];
    // TASK-306: orchestrator-context badge. When this interactive session is
    // a corroborated phase of an `--auto-complete` run, surface the phase
    // index, the `--no-human` scope, and a loud `pause-here` cue — so a user
    // who kicked off `--no-human` expecting to walk away sees that phase 1
    // still needs them. Placed right after the project label so it reads
    // before the role/queue segments. The orchestrator's `.aida/` lives in
    // the main worktree (a phase child runs in a sibling worktree), so
    // corroboration resolves the main root. trace:TASK-306 | ai:claude
    {
        let orch_root = find_main_worktree_root()
            .ok()
            .unwrap_or_else(|| project_root.clone());
        if orchestrator::detect(&orch_root).is_orchestrated() {
            let phase = std::env::var(orchestrator::PHASE_ENV)
                .ok()
                .and_then(|v| v.trim().parse::<u8>().ok());
            let no_human = std::env::var(orchestrator::NO_HUMAN_MODE_ENV)
                .ok()
                .filter(|v| !v.is_empty());
            let badge = OrchestratorBadge::build(phase, no_human.as_deref());
            parts.push(badge.phase.cyan().bold().to_string());
            if let Some(nh) = &badge.no_human {
                parts.push(nh.clone().yellow().bold().to_string());
            }
            parts.push(badge.pause.red().bold().to_string());
        }
    }
    // STORY-706: surface the active focus loudly on every prompt — a
    // persistent focus silently scoping reads is the kubectl-namespace
    // footgun, so the statusline is the always-on reminder. Reads
    // `AIDA_FOCUS` env > `.aida/focus` marker; absent → no segment.
    //
    if let Some(focus) = crate::focus::resolve_focus(&project_root) {
        parts.push(
            format!("focus:{}", truncate(&focus, SCOPE_LABEL_MAX))
                .magenta()
                .bold()
                .to_string(),
        );
    }

    // Resolve the active session lease once: both the @SPEC fallback and
    // the dedicated sess: segment use it, and we want a single canonicalize
    // + read-dir per render. trace:STORY-53 | ai:claude
    let lease = std::env::current_dir()
        .ok()
        .and_then(|cwd| active_lease_for_cwd(&project_root, &cwd));

    {
        let r = &effective_role_name;
        // TASK-244: surface a shell-role vs active-session-role mismatch
        // (e.g. shell persists `implementer` while the only active
        // session is `reviewer`). trace:TASK-244 | ai:claude
        // TASK-645: a *defaulted* (unset) role can't meaningfully mismatch a
        // session lease — suppress the warning and tag the segment
        // `(default)` so the implicit role is visible but unalarming.
        let session_role = lease.as_ref().and_then(|l| l.role.as_deref());
        let warn_enabled = !role_is_default && statusline_role_mismatch_enabled(&project_root);
        let (role_text, role_mismatch) = role_segment_text(r, session_role, warn_enabled);
        let role_text = if role_is_default {
            format!("{} (default)", role_text)
        } else {
            role_text
        };
        let role_segment = if role_mismatch {
            role_text.red().bold().to_string()
        } else {
            role_text.yellow().bold().to_string()
        };
        parts.push(role_segment);
        // @SPEC segment. Default: newest activity entry the active role
        // touched. Source preference: the session-local activity log when
        // cwd is inside an active session lease (STORY-56), else the
        // project-level role's activity stream. Override: with a session
        // active but no role activity yet inside it, fall back to
        // `@<scope>` so the prompt advertises the session anchor instead
        // of a pre-session spec. trace:STORY-55 | ai:claude
        let session_latest: Option<RoleActivity> = lease.as_ref().and_then(|l| {
            load_session_activity(&project_root, &l.id)
                .entries
                .into_iter()
                .find(|e| e.role == *r)
                .map(|e| RoleActivity {
                    spec_id: e.spec_id,
                    action: e.action,
                    at: e.at,
                })
        });
        let role_state = load_role(&project_root, r).ok().map(|(s, _)| s);
        let project_latest = role_state
            .as_ref()
            .and_then(|s| s.activity.first())
            .cloned();
        let latest: Option<RoleActivity> = match (lease.as_ref(), session_latest, project_latest) {
            // In-session: prefer session log; only consider project-level
            // entries that are newer than the lease (i.e. a freshly
            // promoted entry from a prior `session end`).
            (Some(l), Some(s), _) => Some(s).filter(|s| s.at >= l.started_at),
            (Some(l), None, p) => p.filter(|p| p.at >= l.started_at),
            (None, _, p) => p,
        };
        // Raw (untruncated) @SPEC value — the scope/spec the active role
        // is anchored to. Kept raw so the TASK-282 redundancy check
        // compares scopes, not their truncated display forms (a long
        // scope truncated to SCOPE_LABEL_MAX vs SESS_LABEL_MAX would
        // otherwise false-positive as divergence). trace:STORY-55 | ai:claude
        let at_spec: Option<String> = match (latest, lease.as_ref()) {
            (Some(act), Some(l)) if act.at >= l.started_at => Some(act.spec_id.clone()),
            (_, Some(l)) => Some(l.scope.clone()),
            (Some(act), None) => Some(act.spec_id.clone()),
            (None, None) => None,
        };
        if let Some(at_spec) = &at_spec {
            let mut segment = format!("@{}", truncate(at_spec, SCOPE_LABEL_MAX))
                .cyan()
                .bold()
                .to_string();
            // TASK-282: fold the session anchor into the @SPEC segment.
            // When the anchor is redundant with @<scope> — same scope and
            // no batch suffix, the common case — nothing is appended; on
            // divergence it appends ` [sess:<anchor>]` so the prompt still
            // names the owning session. trace:TASK-282 | ai:claude
            if let Some(l) = lease.as_ref() {
                let suffix = derive_session_branch_suffix(&l.scope, &l.branch);
                if let Some(anno) = sess_anchor_annotation(at_spec, &l.scope, &suffix) {
                    segment.push(' ');
                    segment.push_str(&anno.yellow().bold().to_string());
                }
            }
            parts.push(segment);
        }
    }
    if queue_depth > 0 {
        parts.push(format!("q:{}", queue_depth));
    }
    // TASK-648 (ADR-3): the advisor seat owns intake triage, so its statusline
    // surfaces the draft-inbox depth (untriaged drafts to disposition). Only
    // for the advisor — other roles don't clear this queue — and only when
    // non-empty, so a clear inbox stays quiet. trace:TASK-648 | ai:claude
    if effective_role_name == "advisor" {
        let inbox_depth = read_draft_inbox_depth(&cache_path);
        if inbox_depth > 0 {
            parts.push(format!("inbox:{}", inbox_depth).cyan().to_string());
        }
    }
    // STORY-539: surface URGENT unread mailbox messages so an out-of-band
    // escalation/"stop" doesn't sit unseen. Only urgent-unread is loud — a
    // normal informational message stays quiet (the mailbox is a deliberately
    // lightweight channel). Identity is this shell's user id (BUG-89), matching
    // `aida mailbox inbox`. trace:STORY-539 | ai:claude
    if let Some(urgent) = read_urgent_unread_count(&project_root) {
        if urgent > 0 {
            parts.push(
                format!(
                    "{} mail:{}",
                    crate::glyph(crate::glyphs::Glyph::Warning),
                    urgent
                )
                .red()
                .bold()
                .to_string(),
            );
        }
    }
    // Cache freshness: only surface non-fresh states. Fresh is the boring
    // default; rendering it on every prompt is noise.
    //   stale  → red    (cache lags local; next read rebuilds — slow)
    //   behind → red    (local lags origin; run `aida db sync --pull`)
    //   ?      → yellow (origin freshness unknown — STORY-79 not run lately
    //                    or no origin/aida-store ref; informational, not red)
    // trace:STORY-78 | ai:claude
    if let Some(f) = cache_freshness {
        if let Some(label) = f.label() {
            let colored = match f {
                CacheFreshness::Unknown => format!("cache:{}", label).yellow().to_string(),
                _ => format!("cache:{}", label).red().to_string(),
            };
            parts.push(colored);
        }
    }
    // TASK-101: base-freshness — surface when the active lease's branch has
    // fallen behind `origin/main` by N >= threshold commits, so an in-flight
    // session sees it's working on a stale base *during* the session, not just
    // at the queue-work/queue-done gates. Threshold-gated (default 5) + on/off
    // via `[statusline] base_freshness_check`. Cheap + best-effort: only
    // sessions with a live lease on a non-default branch pay the local
    // `git rev-list --count` (NO fetch — reads whatever `origin/main` is locally
    // known), and any git failure stays silent. Loud (red) past 4× threshold.
    // trace:TASK-101 | ai:claude
    if let Some(l) = lease.as_ref() {
        let (bf_enabled, bf_threshold) = statusline_base_freshness_config(&project_root);
        if bf_enabled && !l.branch.is_empty() && l.branch != "main" {
            if let Some(behind) = commits_behind_origin_main(&project_root, &l.branch) {
                if let Some(text) = base_behind_indicator(behind, bf_threshold) {
                    let loud = behind >= bf_threshold.saturating_mul(4);
                    let seg = if loud {
                        text.red().bold().to_string()
                    } else {
                        text.yellow().to_string()
                    };
                    parts.push(seg);
                }
            }
        }
    }
    // sess:<scope> segment — emitted when cwd resolves into an active
    // session lease's worktree. TASK-282: this standalone segment now
    // renders only when there is NO @SPEC segment to fold into (no active
    // role). With a role active, the session anchor lives inside
    // `@<scope> [sess:…]` (rendered above) and a separate `sess:` segment
    // would just repeat it. The TASK-60 batch/branch suffix is preserved
    // either way — without it, three sessions on EPIC-20 (batch3, batch4,
    // batch5) would all render `sess:EPIC-20`, losing the only thing
    // distinguishing them. trace:STORY-53 trace:TASK-60 trace:TASK-282 | ai:claude
    // TASK-645: `role_is_default` is the old `role.is_none()` — no explicit
    // role entered, so the @<scope> role segment carried no session anchor
    // and the standalone `sess:` segment still earns its place.
    if role_is_default {
        if let Some(l) = lease.as_ref() {
            let suffix = derive_session_branch_suffix(&l.scope, &l.branch);
            let label = sess_label_with_suffix(&l.scope, &suffix, SESS_LABEL_MAX);
            parts.push(format!("sess:{}", label).yellow().bold().to_string());
        }
    }
    // wt:<name> segment — emitted only when the session worktree's
    // directory name diverges from the scope-derived slug. `aida session
    // start` auto-names worktrees `<repo>-<slug>`, so the common case
    // renders nothing rather than echoing @<scope>; an explicit `--path`
    // that names the worktree something else is the divergence this
    // segment exists to surface. trace:TASK-282 | ai:claude
    if let Some(l) = lease.as_ref() {
        if let Some(seg) = wt_divergence_segment(&l.worktree_path, &l.slug) {
            parts.push(seg.yellow().bold().to_string());
        }
    }
    // TASK-756/TASK-783: operator-presence segment. Only the effective `away`
    // state is surfaced (a short glyph + word + compact TTL-remaining, e.g.
    // `away 2h`) — `home` is the boring default and stays quiet, matching
    // the cache/freshness "only the non-default is noise-worthy" contract.
    // READ-ONLY: this goes through the non-flipping `statusline_away_remaining`
    // (→ `current_presence`/`effective_presence`), NEVER
    // `auto_flip_if_interactive` — else rendering the statusline on every
    // prompt would itself flip the operator home and `away` could never show.
    // trace:TASK-756 trace:TASK-783
    if let Some(remaining) = presence::statusline_away_remaining(chrono::Utc::now()) {
        parts.push(
            format!(
                "{} away {}",
                crate::glyph(crate::glyphs::Glyph::Away),
                remaining
            )
            .yellow()
            .bold()
            .to_string(),
        );
    }
    // STORY-624: solo-mode segment — surfaced only while active (off is the
    // quiet default, matching the away segment). trace:STORY-624 | ai:claude
    if let Some(marker) = presence::statusline_solo_marker(chrono::Utc::now()) {
        parts.push(marker.magenta().bold().to_string());
    }
    let line = parts.join(&separator);
    if title {
        // trace:TASK-896 — emit the (plain) one-liner as an OSC 2 set-window-title
        // escape. No trailing newline / body text: a prompt that runs
        // `aida statusline --title` updates only the terminal title bar / tmux
        // window name, giving a Codex (or any non-command-footer) session the same
        // AIDA role/queue/inbox context Claude Code shows in its statusLine footer.
        print!("{}", osc_terminal_title(&line));
    } else {
        println!("{line}");
    }
    Ok(())
}

/// Wrap a status line in an OSC 2 set-window-title escape sequence
/// (`ESC ] 2 ; <text> BEL`). Control chars are stripped — they would
/// prematurely terminate the OSC string or corrupt the terminal — so the
/// title carries only the printable AIDA segment. This is the in-agent
/// parity surface for clients whose footer cannot run `aida statusline`
/// directly (e.g. Codex CLI): the AIDA segment rides the terminal title.
// trace:TASK-896
pub(crate) fn osc_terminal_title(line: &str) -> String {
    let safe: String = line.chars().filter(|c| !c.is_control()).collect();
    format!("\x1b]2;{safe}\x07")
}

// ----------------------------------------------------------------------------
// `aida statusline setup` — opt-in AIDA-aware statusline bootstrap.
//
// AIDA's bootstrap goal is to make other projects agent-ready without
// forcing a house style, so enabling the AIDA-aware statusline is a
// deliberate, opt-in step. `aida statusline` (no subcommand) renders the
// one-liner — the quiet default; `aida statusline setup` prints (or, for
// Claude Code, installs) client-appropriate statusline configuration.
// trace:TASK-0414
// ----------------------------------------------------------------------------

// trace:TASK-0414
/// The command Claude Code runs for its `statusLine`. Renders the AIDA
/// one-liner with color forced on (Claude Code pipes the command with no
/// TTY, so `--color=auto` would emit plain text); falls back to the cwd
/// when `aida` is not on PATH or the cwd is outside an AIDA project. Kept
/// in sync with the scaffolder's STATUSLINE_COMMAND so init-scaffolded and
/// setup-installed config agree. No bashisms — runs under /bin/sh (dash).
pub(crate) const STATUSLINE_SETUP_COMMAND: &str =
    "aida statusline --color=always 2>/dev/null || printf '%s' \"$(pwd)\"";

/// Render the Claude Code `settings.json` `statusLine` block as a snippet
/// the user can paste (or that `--install` merges in).
// trace:TASK-0414
pub(crate) fn claude_statusline_block() -> serde_json::Value {
    serde_json::json!({
        "type": "command",
        "command": STATUSLINE_SETUP_COMMAND,
    })
}

/// Print the Claude Code statusline setup guidance (and the JSON snippet).
// trace:TASK-0414
fn print_claude_statusline_setup(settings_path: &std::path::Path) {
    let snippet = serde_json::to_string_pretty(&serde_json::json!({
        "statusLine": claude_statusline_block(),
    }))
    .unwrap_or_else(|_| "{}".to_string());

    println!("Claude Code — command-backed statusLine");
    println!("  Add this to {}:", settings_path.display());
    println!();
    for line in snippet.lines() {
        println!("  {line}");
    }
    println!();
    println!("  Or install it for this project:");
    println!("    aida statusline setup --client claude --install");
    println!();
    println!("  To disable later, remove the \"statusLine\" key from");
    println!(
        "  {} (or run `aida statusline setup --client claude --install` after",
        settings_path.display()
    );
    println!("  deleting it to re-add). Claude Code falls back to its built-in footer.");
}

/// Print the Codex TUI footer guidance. Codex's footer renders built-in
/// item IDs only — it does NOT run `aida statusline` as a command — so we
/// configure companion built-in fields, give the in-agent parity path
/// (the OSC terminal-title segment via `aida statusline --title`), and
/// point at the shell statusline for the full AIDA-aware segment.
// trace:TASK-0414
// trace:TASK-896
fn print_codex_statusline_setup() {
    println!("Codex CLI — built-in TUI footer");
    println!("  Codex's footer renders built-in item IDs; it does not run");
    println!("  `aida statusline` as a command. Configure the companion built-in");
    println!("  fields in `~/.codex/config.toml` (personal) or a trusted project's");
    println!("  `.codex/config.toml` (team):");
    println!();
    println!("  [tui]");
    println!(
        "  status_line = [\"model-with-reasoning\", \"context-remaining\", \"git-branch\", \"current-dir\"]"
    );
    println!();
    println!("  In-agent parity (role / queue depth / inbox depth): Codex's footer");
    println!("  cannot host the `aida statusline` segment, but it honors the terminal");
    println!("  title. Wire `aida statusline --title` into your shell prompt so the");
    println!("  AIDA segment rides the terminal title bar / tmux window name during the");
    println!("  Codex session:");
    println!();
    println!("    # bash (~/.bashrc)");
    println!("    PROMPT_COMMAND='aida statusline --title 2>/dev/null; '\"$PROMPT_COMMAND\"");
    println!();
    println!("    # zsh (~/.zshrc)");
    println!("    precmd() {{ aida statusline --title 2>/dev/null }}");
    println!();
    println!("  For the full AIDA-aware segment as visible text, run");
    println!("  `aida statusline --color=always` anywhere your shell, multiplexer, or");
    println!("  terminal supports a command-backed status line. To disable later, remove");
    println!("  the `[tui] status_line` line (or the whole `[tui]` block) from your Codex");
    println!("  config.toml and drop the `aida statusline --title` prompt hook.");
}

/// Merge the AIDA `statusLine` block into `.claude/settings.json`,
/// preserving any existing keys (hooks, etc.). Creates the file if absent.
/// Returns whether the file was created (vs merged).
// trace:TASK-0414
pub(crate) fn install_claude_statusline(settings_path: &std::path::Path) -> Result<bool> {
    let existed = settings_path.exists();
    let mut root: serde_json::Value = if existed {
        let raw = std::fs::read_to_string(settings_path).with_context(|| {
            format!(
                "reading existing settings.json at {}",
                settings_path.display()
            )
        })?;
        serde_json::from_str(&raw).with_context(|| {
            format!(
                "{} is not valid JSON — fix it by hand, then re-run install",
                settings_path.display()
            )
        })?
    } else {
        if let Some(parent) = settings_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {} for settings.json", parent.display()))?;
        }
        serde_json::json!({})
    };

    let obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{} is not a JSON object", settings_path.display()))?;
    obj.insert("statusLine".to_string(), claude_statusline_block());

    let mut serialized = serde_json::to_string_pretty(&root)?;
    serialized.push('\n');
    std::fs::write(settings_path, serialized)
        .with_context(|| format!("writing {}", settings_path.display()))?;
    Ok(!existed)
}

// trace:TASK-0414
/// Dispatch for `aida statusline setup`.
pub(crate) fn handle_statusline_setup_command(action: &cli::StatuslineAction) -> Result<()> {
    let cli::StatuslineAction::Setup { client, install } = action;

    let project_root = statusline_project_root();
    let settings_path = project_root.join(".claude").join("settings.json");

    match client.as_str() {
        "claude" => {
            if *install {
                let created = install_claude_statusline(&settings_path)?;
                if created {
                    println!(
                        "Created {} with the AIDA statusLine.",
                        settings_path.display()
                    );
                } else {
                    println!(
                        "Merged the AIDA statusLine into {} (existing keys preserved).",
                        settings_path.display()
                    );
                }
                println!(
                    "To disable later, remove the \"statusLine\" key from {}.",
                    settings_path.display()
                );
            } else {
                print_claude_statusline_setup(&settings_path);
            }
        }
        "codex" => {
            if *install {
                anyhow::bail!(
                    "Codex's footer config is hand-edited and cannot be auto-installed.\n\
                     Run `aida statusline setup --client codex` to print the snippet."
                );
            }
            print_codex_statusline_setup();
        }
        // "all" (default): print every supported client's guidance. Install
        // is meaningless for the mixed view, so it prints rather than writes.
        _ => {
            if *install {
                anyhow::bail!(
                    "--install needs a specific client. Use `--client claude --install`."
                );
            }
            print_claude_statusline_setup(&settings_path);
            println!();
            print_codex_statusline_setup();
        }
    }
    Ok(())
}

// trace:TASK-60 | ai:claude
/// TASK-60: pure function deciding what (if anything) to append to
/// the `sess:` segment given the lease scope and branch. Returns an
/// empty string when the branch already matches the slugified scope
/// (no point repeating "epic-20" after "EPIC-20"). Otherwise either
/// `#<suffix>` (when branch shares the scope's slug prefix — the
/// common `epic-N-batchM` pattern) or `@<branch>` (free-form).
pub(crate) fn derive_session_branch_suffix(scope: &str, branch: &str) -> String {
    let scope_slug = slugify(scope);
    if scope_slug.is_empty() || branch == scope_slug {
        return String::new();
    }
    let prefix = format!("{}-", scope_slug);
    if let Some(rest) = branch.strip_prefix(&prefix) {
        if !rest.is_empty() {
            return format!("#{}", rest);
        }
    }
    format!("@{}", branch)
}

// trace:TASK-60 | ai:claude
/// TASK-60: assemble the `sess:` label, fitting scope + suffix into
/// `max_total` characters. Scope is truncated first if the combined
/// length overflows.
pub(crate) fn sess_label_with_suffix(scope: &str, suffix: &str, max_total: usize) -> String {
    if suffix.is_empty() {
        return truncate(scope, max_total).to_string();
    }
    let suffix_len = suffix.chars().count();
    if suffix_len >= max_total {
        // Pathological — suffix alone overflows. Render scope-truncated
        // anyway so the segment still shows context.
        return truncate(scope, max_total).to_string();
    }
    let scope_budget = max_total - suffix_len;
    let scope_part = truncate(scope, scope_budget);
    format!("{}{}", scope_part, suffix)
}

// trace:TASK-282 | ai:claude
/// TASK-282: the `[sess:<anchor>]` annotation folded into the `@SPEC`
/// statusline segment. Returns `None` when the session anchor is
/// redundant with the `@<scope>` label — identical scope AND no
/// batch/branch suffix, the boring common case where a separate `sess:`
/// segment would only repeat `@`. Returns `Some("[sess:<label>]")` on
/// divergence: a child-spec `@` (current scope ≠ session anchor) or a
/// batch suffix the bare `@<scope>` can't carry. Keeping the suffix as a
/// divergence trigger preserves TASK-60's batch disambiguation (three
/// EPIC-20 batches stay distinguishable even when `@` equals the scope).
pub(crate) fn sess_anchor_annotation(
    at_spec: &str,
    sess_scope: &str,
    suffix: &str,
) -> Option<String> {
    if at_spec == sess_scope && suffix.is_empty() {
        return None;
    }
    let label = sess_label_with_suffix(sess_scope, suffix, SESS_LABEL_MAX);
    Some(format!("[sess:{}]", label))
}

// trace:TASK-282 | ai:claude
/// TASK-282: the `wt:<name>` statusline segment. Returns `Some` only when
/// the session worktree's directory name diverges from the scope slug.
/// `aida session start` auto-names worktrees `<repo>-<slug>` (and a bare
/// `<slug>` dir also counts as matching), so the common case renders
/// nothing rather than echoing `@<scope>`. An explicit `--path` that
/// names the worktree something else (e.g. `hot-fix`) is the divergence
/// the segment exists to surface. The matched name is truncated to the
/// `sess:` budget so a long path doesn't blow the line width.
pub(crate) fn wt_divergence_segment(worktree_path: &std::path::Path, slug: &str) -> Option<String> {
    let basename = worktree_path.file_name().and_then(|s| s.to_str())?;
    if slug.is_empty() {
        return None;
    }
    let matches = basename == slug || basename.ends_with(&format!("-{}", slug));
    if matches {
        None
    } else {
        Some(format!("wt:{}", truncate(basename, SESS_LABEL_MAX)))
    }
}
