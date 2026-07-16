// `aida init` command cluster, lifted verbatim out of `main.rs`
// (SPIKE-78 pure-movement extraction). The four init handlers
// (`handle_init_command`, `handle_init_distributed_worktree`,
// `handle_init_post_clone`, `handle_init_distributed_sibling`) plus the
// init-exclusive scaffold helpers (`ensure_plan_template_scaffold`,
// `complete_init_scaffolding`, the `init_scaffold_*` /`commit_init_scaffolding`
// family, `build_initial_scaffold_requirement`, `enqueue_initial_scaffold_task`)
// live here. The SHARED scaffold machinery — discipline/ecosystem-watch pack
// scaffolds, `.aida` gitignore helpers, and the whole memory-pack +
// frontmatter helper block — stays in `main.rs` because `aida scaffold`,
// `aida memories`, `digest`, and their tests also consume it; this module
// reaches all of it via `crate::`.
// trace:SPIKE-78 | ai:claude

use anyhow::Result;
use colored::Colorize;

use crate::*;

pub(crate) fn handle_init_command(
    no_skills: bool,
    agent: &str,
    no_hooks: bool,
    force: bool,
    verbose: bool,
    _name: Option<&str>,
) -> Result<()> {
    eprintln!(
        "{}: --centralized initializes a SQLite-canonical store, which is deprecated.",
        "warning".yellow().bold()
    );
    eprintln!(
        "         Run `aida init` (without flags) to use the recommended git-canonical store."
    );
    eprintln!();

    let db_path = std::path::PathBuf::from("requirements.db");

    // Check existing state
    if db_path.exists() && !force {
        eprintln!(
            "{} AIDA is already initialized in this directory (requirements.db exists).",
            "!".yellow()
        );
        eprintln!("  Use {} to reinitialize.", "--force".bold());
        eprintln!("  To refresh just the scaffolding (CLAUDE.md, .claude/skills/, hooks),");
        eprintln!("  use `aida scaffold apply --force` instead — preserves your store.");
        return Ok(());
    }

    // --force on a populated SQLite store would lose every requirement. Count
    // first; require the user to type "reset" to confirm if non-empty.
    if force && db_path.exists() {
        if let Ok(count) = count_requirements_in_sqlite(&db_path) {
            if count > 0 && !confirm_destructive_reset(count, &db_path)? {
                return Ok(());
            }
        }
    }

    // Create the database
    let storage = Storage::new(db_path.clone());
    let mut store = if db_path.exists() && force {
        storage.load()?
    } else {
        RequirementsStore::default()
    };

    // Seed META requirements
    seed_meta_requirements(&mut store)?;
    storage.save(&store)?;

    // Create docs/plans/
    std::fs::create_dir_all("docs/plans")?;
    ensure_plan_template_scaffold(std::path::Path::new("docs/plans"), force)?;

    // Run the shared workflow scaffolding (skills, hooks, mcp, codex).
    let root = std::env::current_dir().unwrap_or_default();
    complete_init_scaffolding(
        &root,
        &store,
        agent,
        no_skills,
        no_hooks,
        force,
        db_path.clone(),
        "Requirements database (SQLite)",
        verbose,
        // Legacy centralized first-init: commit scaffolding as before (BUG-570
        // suppression is the bootstrap-clone path only). trace:BUG-570
        false,
    )
}

/// Write `docs/plans/_TEMPLATE.md` from the embedded `plan-template.md`
/// template. Idempotent: skips when the file already exists unless `force`
// is set. trace:TASK-92
fn ensure_plan_template_scaffold(plans_dir: &std::path::Path, force: bool) -> Result<()> {
    let dest = plans_dir.join("_TEMPLATE.md");
    if dest.exists() && !force {
        return Ok(());
    }
    if let Some(content) = aida_core::templates::EMBEDDED_TEMPLATES.get("plan-template.md") {
        std::fs::write(&dest, content)?;
    }
    Ok(())
}

/// The canonical set of top-level paths `aida init` writes as scaffolding.
/// This is the ALLOW-LIST for the init self-commit (TASK-631) — the commit
/// stages exactly these and NEVER a bare `git add .`, so an `aida init` run
/// inside an existing repo with unrelated WIP can't sweep the user's changes
/// into the "chore: scaffold AIDA" commit.
///
/// Out of scope by design: user content, the `.aida-store/` worktree (it's a
/// separate orphan branch with its own commits), `.aida/cache.db` and other
/// runtime state (gitignored by the deny-by-default `.aida/*` rule). Only
/// `.aida/config.toml` is the tracked exception under `.aida/`.
// trace:TASK-631 | ai:claude
fn init_scaffold_candidate_paths() -> &'static [&'static str] {
    &[
        ".gitignore",
        ".aida/config.toml",
        ".mcp.json",
        "CLAUDE.md",
        "AGENTS.md",
        ".claude",
        ".codex",
        // trace:TASK-457 | ai:claude
        ".antigravity",
        "docs/plans",
        "docs/aida",
        "docs/agents",
        "docs/extending-skills.md",
        "docs/competitive-analysis",
    ]
}

/// Filter [`init_scaffold_candidate_paths`] to the ones that actually exist
/// on disk under `root`. Pure (modulo filesystem reads) so it can be unit
/// tested against a temp dir. Returns paths relative to `root`, suitable for
// passing straight to `git add`. trace:TASK-631 | ai:claude
fn init_scaffold_commit_paths(root: &std::path::Path) -> Vec<String> {
    init_scaffold_candidate_paths()
        .iter()
        .filter(|p| root.join(p).exists())
        .map(|p| p.to_string())
        .collect()
}

/// Decide whether init should auto-commit its scaffolding (`Some(true)`),
/// never commit (`Some(false)`), or prompt the operator (`None`). Pure so the
/// auto-vs-prompt branch is unit-testable without a real TTY.
///
/// Auto when non-interactive (no TTY: orchestrator / agent / CI / piped) —
/// same non-TTY-fast-path discipline as BUG-422/BUG-407. On a TTY we return
/// `None` so the caller prompts (default-Y) and an operator with unrelated
/// WIP can decline. An explicit `AIDA_INIT_COMMIT_SCAFFOLD` env override wins
/// over the TTY heuristic in both directions (`1`/`true`/`yes`/`on` → auto,
// `0`/`false`/`no`/`off` → never). trace:TASK-631 | ai:claude
fn should_auto_commit_scaffold(stdin_is_tty: bool, env_override: Option<&str>) -> Option<bool> {
    if let Some(raw) = env_override {
        match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => return Some(true),
            "0" | "false" | "no" | "off" => return Some(false),
            _ => {}
        }
    }
    if stdin_is_tty {
        // TTY → caller should prompt (default-Y), not auto-commit.
        None
    } else {
        // Non-interactive → auto-commit.
        Some(true)
    }
}

/// BUG-565: split scaffold paths into `(to_stage, ignored)` by asking git which
/// of them the repo's `.gitignore` covers — `git check-ignore <paths...>` prints
/// exactly the ignored subset (exit 0 = some matched, 1 = none matched, 128 =
/// error). We do NOT pass `--no-index`: a path that is *already tracked* would
/// stage fine via `git add`, so it must not be filtered out; we only want to
/// drop the paths `git add` would actually refuse (untracked + ignored). On any
/// spawn/other error we fail OPEN (treat nothing as ignored) so a check failure
/// never silently drops scaffolding — the subsequent stage will surface it.
// Order within each bucket follows the input. trace:BUG-565 | ai:claude
fn partition_scaffold_paths_by_gitignore(
    root: &std::path::Path,
    paths: &[String],
) -> (Vec<String>, Vec<String>) {
    if paths.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("check-ignore")
        .args(paths)
        .output();
    let ignored: std::collections::HashSet<String> = match output {
        // exit 128 ⇒ git error (e.g. not a repo): fail open, ignore nothing.
        Ok(out) if out.status.code() != Some(128) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => std::collections::HashSet::new(),
    };
    let mut to_stage = Vec::new();
    let mut skipped = Vec::new();
    for p in paths {
        if ignored.contains(p) {
            skipped.push(p.clone());
        } else {
            to_stage.push(p.clone());
        }
    }
    (to_stage, skipped)
}

/// After scaffolding is on disk, commit init's OWN created paths so a fresh
/// clone / session worktree inherits the scaffolding without the operator
/// having to remember the manual `git add . && git commit` step (BUG-445,
/// BUG-433, BUG-73 family). Auto-commits when non-interactive; prompts
/// (default-Y) on a TTY after printing the exact paths it will stage.
///
/// Scoped to [`init_scaffold_commit_paths`] — never `git add .`. Returns
/// `Ok(true)` when a commit was made (so the caller can dedup the onboarding
/// "commit scaffolding" task), `Ok(false)` otherwise. Best-effort: a git
// failure here is a soft note, not a fatal init error. trace:TASK-631 | ai:claude
fn commit_init_scaffolding(root: &std::path::Path) -> Result<bool> {
    use aida_core::git_ops;

    let paths = init_scaffold_commit_paths(root);
    if paths.is_empty() {
        return Ok(false);
    }

    let stdin_is_tty = std::io::stdin().is_terminal();
    let env_override = std::env::var("AIDA_INIT_COMMIT_SCAFFOLD").ok();
    let decision = should_auto_commit_scaffold(stdin_is_tty, env_override.as_deref());

    let proceed = match decision {
        Some(true) => true,
        Some(false) => false,
        None => {
            // TTY: show what will be staged, then prompt default-Y.
            println!();
            println!(
                "  {} will stage these AIDA scaffolding paths:",
                "init".bold()
            );
            for p in &paths {
                println!("    {}", p.dimmed());
            }
            prompt_yes_no("  Commit the AIDA scaffolding now? [Y/n] ", true).unwrap_or(false)
        }
    };

    if !proceed {
        println!(
            "  {} scaffolding left uncommitted — commit it with `git add {} && git commit -m 'chore: scaffold AIDA'`.",
            "Note:".dimmed(),
            paths.join(" "),
        );
        return Ok(false);
    }

    // BUG-565: staging used to be a single all-or-nothing `git add` over EVERY
    // scaffold path. Plain `git add` refuses the WHOLE batch (exit 1) the moment
    // one path is gitignored — and `.claude/` is a very common local-only
    // gitignore entry — so init aborted the commit entirely and stranded the
    // onboarding task, while emitting a dimmed "Note:" that read as informational.
    // Respect the user's ignore choice: filter OUT the paths their `.gitignore`
    // covers (rather than force-adding with `-f`), commit the remainder, and
    // surface each skipped path as a real WARNING so they can decide to un-ignore
    // and re-add. trace:BUG-565 | ai:claude
    let (to_stage, ignored) = partition_scaffold_paths_by_gitignore(root, &paths);

    if !ignored.is_empty() {
        eprintln!(
            "  {} skipped {} scaffolding path{} your .gitignore covers (not committed):",
            "Warning:".yellow().bold(),
            ignored.len(),
            if ignored.len() == 1 { "" } else { "s" },
        );
        for p in &ignored {
            eprintln!("    {}", p.yellow());
        }
        eprintln!(
            "  {} un-ignore a path and re-stage it manually with `git add -f {}` if you want it tracked.",
            "→".dimmed(),
            ignored.join(" "),
        );
    }

    if to_stage.is_empty() {
        // Everything init created is gitignored — nothing to commit. Honest
        // warning above already told the user which paths and why.
        eprintln!(
            "  {} all AIDA scaffolding paths are gitignored — nothing committed.",
            "Warning:".yellow().bold(),
        );
        return Ok(false);
    }

    let path_refs: Vec<&str> = to_stage.iter().map(|s| s.as_str()).collect();
    if let Err(e) = git_ops::add(root, &path_refs) {
        eprintln!(
            "  {} could not stage scaffolding paths: {}",
            "Warning:".yellow().bold(),
            e
        );
        return Ok(false);
    }
    match git_ops::commit(root, "chore: scaffold AIDA") {
        Ok(true) => {
            println!(
                "  {} saved your AIDA setup",
                crate::glyph(crate::glyphs::Glyph::Check).green()
            );
            Ok(true)
        }
        // Nothing staged was new (already tracked + unchanged). Not an error.
        Ok(false) => Ok(false),
        Err(e) => {
            eprintln!(
                "  {} could not commit scaffolding: {}",
                "Warning:".yellow().bold(),
                e
            );
            Ok(false)
        }
    }
}

/// Workflow scaffolding shared by all `aida init` modes — builds skills,
/// commands, hooks, MCP integration, etc. Called by both centralized and
/// distributed init paths after their respective storage setup is complete.
// trace:EPIC-1-001 | ai:claude
#[allow(clippy::too_many_arguments)]
fn complete_init_scaffolding(
    root: &std::path::Path,
    store: &RequirementsStore,
    agent: &str,
    no_skills: bool,
    no_hooks: bool,
    force: bool,
    db_path: std::path::PathBuf,
    storage_label: &str,
    verbose: bool,
    // BUG-570: when true, write the scaffold files but do NOT create a
    // code-branch commit. The bootstrap-clone path (existing AIDA store on
    // origin) passes this so `aida init` on a clone never auto-commits a
    // scaffold dump to the shared default branch — the operator commits
    // deliberately if they want. Genuinely-new first-init passes false and
    // commits its own scaffolding as before. trace:BUG-570 | ai:claude
    suppress_scaffold_commit: bool,
) -> Result<()> {
    // Build ScaffoldConfig with escape hatches
    let mut config = ScaffoldConfig::default();
    match agent {
        "claude" => {
            config.generate_agents_md = false;
            config.generate_codex_skills = false;
            // A Claude-only project needs no Codex MCP registration; the
            // .codex/config.toml is the Codex-side parallel to .mcp.json and
            // is skipped here alongside the Codex skills. trace:TASK-0424 | ai:claude
            config.generate_codex_config = false;
            // .antigravity/ mirrors the non-Claude .codex/ dir; the
            // Claude-only profile skips both. trace:TASK-457 | ai:claude
            config.generate_antigravity_skills = false;
        }
        "codex" => {
            config.generate_claude_md = false;
            config.generate_commands = false;
            config.generate_skills = false;
            config.generate_claude_code_hooks = false;
        }
        "both" => {}
        _ => {
            anyhow::bail!(
                "Invalid --agent value '{}'. Use claude, codex, or both.",
                agent
            );
        }
    }

    if no_skills {
        config.generate_skills = false;
        config.generate_commands = false;
        config.generate_codex_skills = false;
        // --no-skills skips every agent-skill dir consistently, including
        // the new .antigravity/skills/. trace:TASK-457 | ai:claude
        config.generate_antigravity_skills = false;
        config.include_aida_req_skill = false;
        config.include_aida_plan_skill = false;
        config.include_aida_implement_skill = false;
        config.include_aida_capture_skill = false;
        config.include_aida_docs_skill = false;
        config.include_aida_release_skill = false;
        config.include_aida_evaluate_skill = false;
        config.include_aida_commit_skill = false;
        config.include_aida_sync_skill = false;
        config.include_aida_test_skill = false;
        config.include_aida_review_skill = false;
        config.include_aida_onboard_skill = false;
        config.include_aida_sprint_skill = false;
        config.include_aida_search_skill = false;
        config.include_aida_standup_skill = false;
        config.include_aida_import_plan_skill = false;
    }
    if no_hooks {
        config.generate_git_hooks = false;
        config.generate_claude_code_hooks = false;
        config.include_commit_msg_hook = false;
        config.include_pre_commit_hook = false;
        config.include_validate_commit_hook = false;
        config.include_track_commits_hook = false;
    }

    // Run scaffold
    let config_for_output = config.clone();
    let mut scaffolder = Scaffolder::with_database(root.to_path_buf(), config, db_path.clone());
    let preview = scaffolder.preview(store);

    let mut _created_count = 0;
    let mut updated_count = 0;
    let mut skipped_count = 0;

    for artifact in &preview.artifacts {
        let full_path = root.join(&artifact.path);
        let exists = full_path.exists();

        if exists && !force {
            if artifact.path == std::path::Path::new(".git/hooks/pre-commit") {
                let is_managed = if let Ok(content) = std::fs::read_to_string(&full_path) {
                    content.contains("AIDA Generated") || content.contains("Generated by AIDA")
                } else {
                    false
                };
                if is_managed {
                    // It is AIDA-managed, so do NOT skip. We want to update it!
                } else {
                    eprintln!(
                        "{} Warning: .git/hooks/pre-commit exists and contains custom user edits. Skipping. Use --force to overwrite.",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                    );
                    skipped_count += 1;
                    continue;
                }
            } else {
                skipped_count += 1;
                continue;
            }
        }

        // Create parent directories if needed
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full_path, &artifact.content)?;
        // Hooks (and any executable shell-style scripts) need the exec
        // bit; without it git silently ignores commit-msg hooks and the
        // user gets a confusing "hook was ignored" warning on every
        // commit. trace:BUG-21 | ai:claude
        ensure_executable_if_hook(&artifact.path, &full_path);

        if exists {
            updated_count += 1;
        } else {
            _created_count += 1;
        }
    }

    // Scaffold the discipline pack (docs/aida/discipline/) — generic
    // AIDA-using guidance, written for every init mode. trace:STORY-255 | STORY-443
    let discipline_written = ensure_discipline_pack_scaffold(root, force).unwrap_or(0);

    // Scaffold a starter ecosystem-watch log so a fresh project's first
    // `scripts/release.sh minor` doesn't trip the missing-file warning
    // path. trace:TASK-126
    let ecosystem_watch_written = ensure_ecosystem_watch_scaffold(root, force).unwrap_or(false);

    // Auto-configure Codex MCP if codex is installed
    if std::process::Command::new("codex")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        // Check if already configured
        let mcp_list = std::process::Command::new("codex")
            .args(["mcp", "list"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();

        if !mcp_list.contains("aida") {
            match std::process::Command::new("codex")
                .args(["mcp", "add", "aida", "--", "aida", "mcp-serve"])
                .output()
            {
                Ok(o) if o.status.success() => {
                    println!(
                        "{} Codex CLI MCP server configured",
                        crate::glyph(crate::glyphs::Glyph::Check).green()
                    );
                }
                _ => {
                    // Silently skip — codex mcp add may fail for various reasons
                }
            }
        }
    }

    // Print post-init message. Brief by default; --verbose for the full
    // file inventory + per-agent hint blocks. trace:BUG-19 | ai:claude
    println!();
    println!(
        "{}",
        format!(
            "AIDA initialized {}",
            crate::glyph(crate::glyphs::Glyph::Check)
        )
        .green()
        .bold()
    );

    if verbose {
        println!();
        println!("  {}:", "Created".bold());
        println!("    {}", storage_label);
        if config_for_output.generate_claude_md {
            println!(
                "    {}{}Claude Code MCP integration",
                ".mcp.json".white().bold(),
                " ".repeat(29)
            );
            println!(
                "    {}{}Project context for AI sessions",
                "CLAUDE.md".white().bold(),
                " ".repeat(29)
            );
        }
        if config_for_output.generate_agents_md {
            println!(
                "    {}{}Project context for Codex-compatible agents",
                "AGENTS.md".white().bold(),
                " ".repeat(29)
            );
        }
        if config_for_output.generate_skills {
            println!(
                "    {}{}Workflow skills",
                ".claude/skills/".white().bold(),
                " ".repeat(23)
            );
        }
        if config_for_output.generate_commands {
            println!(
                "    {}{}Slash commands",
                ".claude/commands/".white().bold(),
                " ".repeat(21)
            );
        }
        if config_for_output.generate_codex_skills {
            println!(
                "    {}{}Workflow skills (Codex-compatible)",
                ".codex/skills/".white().bold(),
                " ".repeat(24)
            );
        }
        // trace:TASK-457 | ai:claude
        if config_for_output.generate_antigravity_skills {
            println!(
                "    {}{}Workflow skills (Antigravity-compatible)",
                ".antigravity/skills/".white().bold(),
                " ".repeat(18)
            );
        }
        if !no_hooks && config_for_output.generate_claude_code_hooks {
            println!(
                "    {}{}Commit validation hooks",
                ".claude/hooks/".white().bold(),
                " ".repeat(24)
            );
        }
        if !no_hooks && config_for_output.generate_git_hooks {
            println!(
                "    {}{}Git commit-msg hook",
                ".git/hooks/commit-msg".white().bold(),
                " ".repeat(18)
            );
        }
        println!(
            "    {}{}Implementation plan archive",
            "docs/plans/".white().bold(),
            " ".repeat(27)
        );
        println!(
            "    {}{}AIDA-using discipline guides",
            "docs/aida/discipline/".white().bold(),
            " ".repeat(17)
        );
    } else {
        // Brief: one line of storage location + counts.
        println!("  Storage: {}", storage_label.dimmed());
    }

    // The discipline pack + ecosystem-watch log are real, but announcing them
    // at minute one is noise for a newcomer — they didn't ask for 21 guides or a
    // competitive-analysis log and don't yet know what they're for. Surface the
    // count only under --verbose; the files are on disk either way.
    // trace:BUG-19 | ai:claude
    if verbose {
        if discipline_written > 0 {
            println!(
                "  {} discipline guide{} scaffolded to {}",
                discipline_written.to_string().green(),
                if discipline_written == 1 { "" } else { "s" },
                "docs/aida/discipline/".dimmed(),
            );
        }
        if ecosystem_watch_written {
            println!(
                "  starter ecosystem-watch log scaffolded to {}",
                "docs/competitive-analysis/ecosystem-watch.md".dimmed(),
            );
        }
    }

    if skipped_count > 0 {
        println!(
            "  {} files skipped (already exist, use --force to overwrite)",
            skipped_count.to_string().yellow(),
        );
    }
    if updated_count > 0 {
        println!("  {} files updated", updated_count.to_string().blue(),);
    }

    println!();
    // TASK-732: teach the link bridge, not just the four verbs. The novice's
    // confusion sits between `aida list` and `aida done` — AIDA doesn't build
    // for them; the value is that referencing the id in a commit (or a
    // // trace: comment) makes `aida show` reveal the code wired to the spec.
    // The dim signpost line names that bridge; `aida show` is reframed from
    // "recall why" to the payoff. trace:TASK-732 | ai:claude
    println!(
        "  {} The loop: {} → build → {} → done.",
        crate::glyph(crate::glyphs::Glyph::FlowActive)
            .green()
            .bold(),
        "capture".bold(),
        "link".bold()
    );
    println!(
        "    {}{}capture a task{}",
        "aida add \"Build login page\"".cyan(),
        " ".repeat(3),
        "   → TASK-1".dimmed()
    );
    println!(
        "    {}{}see what's on your plate",
        "aida list".cyan(),
        " ".repeat(21)
    );
    println!(
        "    {}",
        "… now build it in your editor, and put (TASK-1) in your commit message …".dimmed()
    );
    println!(
        "    {}{}mark it finished",
        "aida done TASK-1".cyan(),
        " ".repeat(14)
    );
    // BUG-585: `aida done` is advisor-gated off a TTY, so a scripted/agent
    // first-user (a primary intended caller) would hit the gate on this taught
    // step. Name the non-interactive escape hatch inline so it doesn't
    // dead-end. trace:BUG-585 | ai:claude
    println!(
        "    {}",
        "      (scripts/agents: prefix AIDA_SESSION_ROLE=advisor)".dimmed()
    );
    println!(
        "    {}{}see your commit linked to the task — {}",
        "aida show TASK-1".cyan(),
        " ".repeat(14),
        "that's the point".bold()
    );

    // TASK-645: surface the role model at the onboarding moment so there is
    // no undefined-role window. The default is read-side (`AIDA_SESSION_ROLE`
    // unset → implementer); init can't set the env var from a subprocess, so
    // it names the default and the switch command rather than entering one.
    // TASK-645 / ADR-2: name the default seat so there's no undefined-role
    // window — but for a newcomer that's one gentle line, not a lecture on the
    // role model. The switch command + the advisor/reviewer seats surface later
    // (`aida role --help`, the discipline pack) once the project grows into them.
    // trace:TASK-645 | ai:claude
    println!();
    println!(
        "  You're set up as the {} (the default) — you can ignore roles until your project grows.",
        "implementer".green().bold()
    );

    if !verbose {
        println!();
        // BUG-38: the prior hint pointed at re-running `aida init
        // --verbose`, but that fails post-init with "already initialized".
        // Steer the user to `aida scaffold status --verbose` instead — it
        // works any time and reports what's actually on disk.
        // trace:BUG-38 | ai:claude
        println!(
            "  {}",
            "(later: `aida scaffold status --verbose` to see every scaffolded file)".dimmed()
        );
    } else if config_for_output.generate_claude_md {
        println!();
        println!("  {}:", "In Claude Code".bold());
        println!(
            "    {}{}Interactive project walkthrough",
            "/aida-onboard".cyan(),
            " ".repeat(15)
        );
        println!(
            "    {}{}Add a requirement",
            "/aida-req".cyan(),
            " ".repeat(19)
        );
        println!(
            "    {}{}See all available skills",
            "/aida-".cyan(),
            " ".repeat(22)
        );
    }
    println!();

    // TASK-865: report the bubblewrap OS-sandbox availability so a new user
    // learns up front whether userns confinement is available on this host.
    // Informational only — never a prompt, never blocks init, and never enables
    // `os_wrap` (that knob is opt-in / off by default). trace:TASK-865 | ai:claude
    {
        let avail = crate::session::bwrap_availability();
        let glyph = match avail {
            crate::session::BwrapAvailability::Ok => {
                crate::glyph(crate::glyphs::Glyph::Check).green()
            }
            crate::session::BwrapAvailability::NotInstalled => {
                crate::glyph(crate::glyphs::Glyph::Bullet).dimmed()
            }
            crate::session::BwrapAvailability::UsernsBlocked { .. } => {
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            }
        };
        println!("  {} {}", glyph, bwrap_status_line());
        // STORY-665: when confinement isn't ready, point at the guided setup
        // command instead of just naming the knob — the operator needs a path,
        // not a hint. trace:STORY-665 | ai:claude
        if avail == crate::session::BwrapAvailability::Ok {
            println!(
                "    {}",
                "OS sandbox is opt-in (off by default); enable via [contained] os_wrap".dimmed()
            );
        } else {
            println!(
                "    {}",
                "OS sandbox is opt-in (off by default); run `aida doctor --fix-sandbox` for the setup steps"
                    .dimmed()
            );
        }
        println!();
    }

    // TASK-860: report the forge CLI (`gh`/`glab`) availability so a new user
    // learns up front whether PR/review automation will work — instead of the
    // "silent until you hit a `pr` command" failure (`aida pr auto-queue-review`
    // fails hard when `gh` is absent). The forge kind is the one already written
    // to `.aida/config.toml` (origin-host auto-detect / explicit override), so we
    // only ever name `glab` to a GitLab project, never nag a GitHub user about
    // it. Informational only — never a prompt, never blocks init.
    // trace:TASK-860 | ai:claude
    {
        let (kind, msg) = crate::forge::forge_cli_status(root);
        let glyph = if kind == crate::forge::ForgeKind::None {
            crate::glyph(crate::glyphs::Glyph::Bullet).dimmed()
        } else if kind.cli_on_path() {
            crate::glyph(crate::glyphs::Glyph::Check).green()
        } else {
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        };
        println!("  {} {}", glyph, msg);
        println!();
    }

    // TASK-631: commit init's OWN scaffolding now that every scaffolded path
    // is on disk — auto when non-interactive, prompt default-Y on a TTY,
    // scoped to init-created paths (never `git add .`). This dissolves the
    // onboarding "remember to commit the scaffolding" footgun (BUG-445) and
    // the downstream "fresh clone / session worktree lacks the scaffolding"
    // failure (BUG-433, BUG-73 family). trace:TASK-631 | ai:claude
    //
    // BUG-570: on a bootstrap-clone (existing AIDA store on origin) we suppress
    // this entirely. A clone re-scaffolds regenerable files the upstream tree
    // already carries (and dirs the upstream deliberately tracks as symlinks /
    // doesn't track at all); auto-committing them — silently, when init runs
    // headless with no TTY — produced a divergent "chore: scaffold AIDA" dump
    // that polluted the shared default branch on the next push. Write the files,
    // leave the working tree uncommitted, and tell the operator. The commit is
    // theirs to make deliberately (or opt in with `aida init --commit-scaffold`).
    // trace:BUG-570 | ai:claude
    let scaffolding_committed = if suppress_scaffold_commit {
        let paths = init_scaffold_commit_paths(root);
        if !paths.is_empty() {
            println!(
                "  {} wrote AIDA scaffold files and left them uncommitted — commit deliberately with `git add {} && git commit` if you want them on this clone's branch.",
                "Note:".dimmed(),
                paths.join(" "),
            );
        }
        false
    } else {
        commit_init_scaffolding(root).unwrap_or(false)
    };

    // trace:TASK-510 | ai:antigravity
    // DEDUP (TASK-631): when init committed the scaffolding itself, skip
    // enqueuing the "Commit AIDA scaffolding" onboarding task — the first
    // user shouldn't be told to do an already-done step.
    //
    // BUG-570: when we deliberately suppressed the commit (bootstrap-clone),
    // also skip the onboarding task — a clone of an already-set-up project
    // shouldn't be nudged to commit a scaffold dump to the shared branch; the
    // uncommitted-files note above already covers the deliberate-commit option.
    if !scaffolding_committed && !suppress_scaffold_commit {
        if let Err(e) = enqueue_initial_scaffold_task(root, &db_path) {
            eprintln!(
                "  {} could not enqueue initial scaffolding task: {}",
                "Note:".dimmed(),
                e
            );
        }
    }

    // STORY-700: kick off the passive first-run hint chain — this prints the
    // "file your first spec" nudge and records that the arc has started, so the
    // subsequent add / queue-done / pull anchors carry it forward. Idempotent:
    // a re-run of init over a started-or-finished arc is a silent no-op.
    // trace:STORY-700 | ai:claude
    first_run::after_init(root);

    Ok(())
}

/// Build the seeded onboarding "Commit AIDA scaffolding" requirement (sans the
/// spec-id-dependent description, which the caller fills once an id is assigned).
///
/// BUG-445: the durable guardrail lives here. A scaffolding-commit task acts on
/// the PRIMARY worktree's uncommitted state (the freshly-scaffolded `.gitignore`,
/// `.aida/config.toml`, `.claude/*`). It must NEVER route through `aida queue
/// work` worktree isolation: a worktree branched off the initial commit lacks
/// the scaffolding entirely, and a bare `git add .` there (no `.gitignore`
/// present) would embed the `.aida-store` gitlink + machine-specific
/// cache/session symlinks into history. Flagging it `human_only` makes the
/// pickability gate (`aida_core::pickability`) refuse it at every queue-work
/// pickup site — head pickup, batch drain, and cluster drain — so it stays
/// primary-worktree-only. The human (or any non-isolated session) marks it done
/// with `aida queue done` after committing in the canonical repo.
// trace:BUG-445 | ai:claude
fn build_initial_scaffold_requirement() -> aida_core::Requirement {
    use aida_core::{Requirement, RequirementPriority, RequirementStatus, RequirementType};

    let mut req = Requirement::new(
        "Commit AIDA scaffolding (initial setup)".to_string(),
        "".to_string(),
    );
    req.req_type = RequirementType::Task;
    req.status = RequirementStatus::Approved;
    req.priority = RequirementPriority::High;
    req.tags.insert("from-aida-init".to_string());
    req.tags.insert("first-task".to_string());
    req.tags.insert("scaffolding".to_string());
    // trace:BUG-445 | ai:claude — primary-worktree-only guardrail (see fn doc).
    req.human_only = true;
    req
}

// trace:TASK-510 | ai:antigravity
fn enqueue_initial_scaffold_task(root: &std::path::Path, db_path: &std::path::Path) -> Result<()> {
    use aida_core::Storage;

    let resolved_db_path = root.join(db_path);
    let storage = Storage::new(&resolved_db_path);
    let mut store = storage.load()?;

    let already_exists = store
        .requirements
        .iter()
        .any(|r| r.tags.contains("from-aida-init"));

    if already_exists {
        return Ok(());
    }

    let req = build_initial_scaffold_requirement();

    store.add_requirement_with_id(req, None, Some("task"));

    let (requirement_uuid, spec_id_display) = if let Some(last) = store.requirements.last_mut() {
        let spec_id = last.spec_id.as_deref().unwrap_or("TASK-1").to_string();
        last.description = format!(
            "After aida init, the scaffolded files are untracked. Stage only AIDA's own paths (never `git add .` in a repo with unrelated work): git add .gitignore .aida/config.toml .mcp.json CLAUDE.md AGENTS.md .claude docs/plans docs/aida && git commit -m 'chore: scaffold AIDA'. The .gitignore deny-by-default rules (.aida/* + !.aida/config.toml) keep runtime state out. Run 'aida queue done {}' after the commit lands.",
            spec_id
        );

        println!(
            "  {} onboarding task enqueued: {}",
            "Enqueued".green(),
            spec_id.cyan()
        );
        (last.id, spec_id)
    } else {
        return storage.save(&store);
    };

    storage.save(&store)?;

    // TASK-1-097 (BUG-386 sibling): also push the onboarding task onto the
    // user's implementer queue so 'aida queue work' picks it up + 'aida
    // status' shows it as queue head. Before this, the spec was Approved
    // but invisible to queue surfaces — operator saw "Queue: (empty)" with
    // the onboarding task only in "Recent activity," which is misleading
    // first-impression UX. trace:TASK-1-097 | ai:claude
    let user_id = current_user_id(None);
    let queue_entry = aida_core::QueueEntry {
        user_id: user_id.clone(),
        requirement_id: requirement_uuid,
        // i64::MAX = "append to bottom" sentinel; the git backend resolves
        // to max_position + 1000 (STORY-72). New project, no other queue
        // items, so this lands at the head naturally.
        position: i64::MAX,
        added_by: user_id,
        note: Some(format!(
            "Auto-queued by 'aida init' onboarding task ({})",
            spec_id_display
        )),
        added_at: chrono::Utc::now(),
        for_role: Some("implementer".to_string()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    // Best-effort queue_add — the spec exists either way; queue-add failure
    // is a softer error than the substrate write being unrecoverable.
    if let Err(e) = storage.queue_add(queue_entry) {
        eprintln!(
            "  {} could not push onboarding task onto implementer queue: {}",
            "Warning:".yellow(),
            e
        );
    }
    Ok(())
}

#[cfg(test)]
mod task_510_init_scaffold_task_tests {
    use super::*;
    use aida_core::{RequirementPriority, RequirementStatus, RequirementType, Storage};
    use tempfile::TempDir;

    #[test]
    fn test_enqueue_initial_scaffold_task_success_and_idempotency() {
        let tmp = TempDir::new().unwrap();
        let db_path = std::path::Path::new(".aida/cache.db");
        let full_db_dir = tmp.path().join(".aida");
        std::fs::create_dir_all(&full_db_dir).unwrap();

        // 1. Initial enqueueing
        enqueue_initial_scaffold_task(tmp.path(), db_path).unwrap();

        // Load and assert
        let storage = Storage::new(tmp.path().join(db_path));
        let store = storage.load().unwrap();
        assert_eq!(store.requirements.len(), 1);

        let req = &store.requirements[0];
        assert_eq!(req.title, "Commit AIDA scaffolding (initial setup)");
        assert_eq!(req.req_type, RequirementType::Task);
        assert_eq!(req.status, RequirementStatus::Approved);
        assert_eq!(req.priority, RequirementPriority::High);
        assert!(req.tags.contains("from-aida-init"));
        assert!(req.tags.contains("first-task"));
        assert!(req.tags.contains("scaffolding"));

        let spec_id = req.spec_id.as_deref().unwrap_or("TASK-1");
        assert!(req
            .description
            .contains("After aida init, the scaffolded files are untracked."));
        assert!(req
            .description
            .contains(&format!("Run 'aida queue done {}'", spec_id)));

        // 2. Idempotency check: run it again and verify we don't duplicate
        enqueue_initial_scaffold_task(tmp.path(), db_path).unwrap();
        let store2 = storage.load().unwrap();
        assert_eq!(
            store2.requirements.len(),
            1,
            "Idempotency failed: task was duplicated"
        );
    }

    /// BUG-445: the durable guardrail. The seeded scaffolding-commit task
    /// must be flagged `human_only` so `aida queue work` never worktree-
    /// isolates it. A worktree branched off the initial commit lacks the
    /// uncommitted scaffolding entirely, and a bare `git add .` there would
    /// embed the `.aida-store` gitlink + machine-specific cache/session
    /// symlinks into history. `human_only` makes the pickability gate refuse
    /// the spec at every queue-work pickup site, keeping it primary-worktree-
    /// only.
    ///
    /// This asserts on the builder (`build_initial_scaffold_requirement`) +
    /// the pickability gate directly, NOT on a storage round-trip: the legacy
    /// SQLite cache projection is lossy on `human_only` (the canonical
    /// git-backend YAML round-trip preserves it, per
    /// `RequirementsStore`/`models.rs`), so testing through `Storage::load`
    /// on a `.db` path would assert the cache's lossiness rather than the
    // seeding decision. trace:BUG-445 | ai:claude
    #[test]
    fn scaffold_task_is_primary_worktree_only_and_not_pickable() {
        use aida_core::pickability::{pickability, BlockedReason, Pickability};
        use aida_core::RequirementsStore;

        let req = build_initial_scaffold_requirement();

        // The guardrail flag itself.
        assert!(
            req.human_only,
            "scaffolding-commit task must be human_only so queue-work never \
             worktree-isolates it (BUG-445)"
        );

        // And the consequence: the pickability gate (consulted by every
        // queue-work pickup site — head pickup, batch drain, cluster drain)
        // refuses it, so it can never route through worktree isolation.
        let mut store = RequirementsStore::default();
        store.requirements.push(req.clone());
        assert_eq!(
            pickability(&req, &store),
            Pickability::Blocked(BlockedReason::HumanOnly),
            "scaffolding-commit task must be un-pickable (human-only) so \
             `aida queue work` cannot isolate it into a worktree (BUG-445)"
        );
    }

    /// BUG-445 guard (c): the seeded task instruction must never emit a bare
    /// `git add .` — it stages only AIDA's own paths, with `.gitignore`
    /// listed first so deny-by-default rules apply. Asserts on the persisted
    /// description (which the cache DOES round-trip, unlike `human_only`).
    // trace:BUG-445 | ai:claude
    #[test]
    fn scaffold_task_instruction_never_bare_git_add_dot() {
        let tmp = TempDir::new().unwrap();
        let db_path = std::path::Path::new(".aida/cache.db");
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();

        enqueue_initial_scaffold_task(tmp.path(), db_path).unwrap();

        let storage = Storage::new(tmp.path().join(db_path));
        let store = storage.load().unwrap();
        let desc = &store.requirements[0].description;

        // Match the bare form precisely: `git add .gitignore ...` is the
        // *correct* scoped form and legitimately starts with the same chars.
        assert!(
            !desc.contains("git add . "),
            "instruction must never emit a bare `git add .` (BUG-445 c); got:\n{desc}"
        );
        assert!(
            !desc.contains("git add .\n") && !desc.ends_with("git add ."),
            "instruction must never emit a bare `git add .` (BUG-445 c); got:\n{desc}"
        );
        assert!(
            desc.contains("git add .gitignore"),
            "instruction must stage scoped paths starting with .gitignore (BUG-445 c)"
        );
    }
}

// trace:TASK-631 | ai:claude
#[cfg(test)]
mod task_631_init_self_commit_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn commit_paths_filters_to_existing_only_and_never_bare_dot() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // Nothing written yet → empty list.
        assert!(init_scaffold_commit_paths(root).is_empty());

        // Write a representative subset of init-owned paths.
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(root.join(".aida/config.toml"), "x").unwrap();
        std::fs::write(root.join(".gitignore"), "x").unwrap();
        std::fs::write(root.join("CLAUDE.md"), "x").unwrap();
        std::fs::create_dir_all(root.join(".claude/skills")).unwrap();
        std::fs::write(root.join(".claude/skills/foo.md"), "x").unwrap();
        std::fs::create_dir_all(root.join("docs/aida")).unwrap();

        // An UNRELATED user file must NOT appear in the staged set.
        std::fs::write(root.join("user_wip.rs"), "x").unwrap();

        let paths = init_scaffold_commit_paths(root);
        assert!(paths.contains(&".aida/config.toml".to_string()));
        assert!(paths.contains(&".gitignore".to_string()));
        assert!(paths.contains(&"CLAUDE.md".to_string()));
        assert!(paths.contains(&".claude".to_string()));
        assert!(paths.contains(&"docs/aida".to_string()));

        // Candidates that were never written are filtered out.
        assert!(!paths.contains(&"AGENTS.md".to_string()));
        assert!(!paths.contains(&".codex".to_string()));

        // CRITICAL invariant: never a bare "." and never the user's WIP.
        assert!(!paths.iter().any(|p| p == "."));
        assert!(!paths.iter().any(|p| p == "user_wip.rs"));

        // Every staged path is one of the known candidates.
        let candidates = init_scaffold_candidate_paths();
        for p in &paths {
            assert!(
                candidates.contains(&p.as_str()),
                "staged path {p} is not in the init allow-list"
            );
        }
    }

    #[test]
    fn auto_vs_prompt_decision() {
        // Non-interactive (no TTY) → auto-commit.
        assert_eq!(should_auto_commit_scaffold(false, None), Some(true));
        // Interactive (TTY) → prompt (None).
        assert_eq!(should_auto_commit_scaffold(true, None), None);

        // Env override forces auto even on a TTY.
        assert_eq!(
            should_auto_commit_scaffold(true, Some("1")),
            Some(true),
            "env=1 should force auto"
        );
        assert_eq!(should_auto_commit_scaffold(true, Some("true")), Some(true));
        assert_eq!(should_auto_commit_scaffold(true, Some("YES")), Some(true));
        assert_eq!(should_auto_commit_scaffold(true, Some(" on ")), Some(true));

        // Env override forces never even when non-interactive.
        assert_eq!(
            should_auto_commit_scaffold(false, Some("0")),
            Some(false),
            "env=0 should force never"
        );
        assert_eq!(
            should_auto_commit_scaffold(false, Some("false")),
            Some(false)
        );
        assert_eq!(should_auto_commit_scaffold(false, Some("no")), Some(false));
        assert_eq!(should_auto_commit_scaffold(false, Some("off")), Some(false));

        // Unrecognized env value falls through to the TTY heuristic.
        assert_eq!(
            should_auto_commit_scaffold(false, Some("maybe")),
            Some(true)
        );
        assert_eq!(should_auto_commit_scaffold(true, Some("maybe")), None);
    }

    fn git_in(root: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("git {args:?} failed to spawn: {e}"));
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// BUG-565: the resilient-staging partition must drop the gitignored path
    /// and keep the rest — never abort the whole batch on one ignored path.
    /// `.claude` is the canonical local-only gitignore entry that used to
    // strand the onboarding commit. trace:BUG-565 | ai:claude
    #[test]
    fn partition_drops_gitignored_paths_keeps_remainder() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        git_in(root, &["init", "-q"]);
        // The user's .gitignore covers .claude (a very common local-only entry).
        std::fs::write(root.join(".gitignore"), ".claude/\n").unwrap();
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(root.join(".claude/x"), "x").unwrap();
        std::fs::write(root.join("CLAUDE.md"), "x").unwrap();

        let paths = vec![
            ".gitignore".to_string(),
            "CLAUDE.md".to_string(),
            ".claude".to_string(),
        ];
        let (to_stage, ignored) = partition_scaffold_paths_by_gitignore(root, &paths);

        // The ignored path is filtered out (NOT force-added — respect the
        // user's ignore choice) and surfaced for a warning.
        assert_eq!(ignored, vec![".claude".to_string()]);
        // The remainder is staged, in input order.
        assert_eq!(
            to_stage,
            vec![".gitignore".to_string(), "CLAUDE.md".to_string()]
        );
    }

    /// BUG-565 end-to-end: a scaffold set containing one gitignored path must
    /// still COMMIT the non-ignored remainder (init no longer aborts the whole
    /// commit on one ignored path) and must NOT commit the ignored path.
    // trace:BUG-565 | ai:claude
    #[test]
    fn commit_scaffolding_commits_remainder_when_one_path_is_gitignored() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        git_in(root, &["init", "-q"]);
        git_in(root, &["config", "user.email", "t@t"]);
        git_in(root, &["config", "user.name", "t"]);
        git_in(root, &["commit", "--allow-empty", "-q", "-m", "root"]);

        // .gitignore covers .claude; init writes both .claude (ignored) and the
        // non-ignored scaffolding (.aida/config.toml, CLAUDE.md, .gitignore).
        std::fs::write(root.join(".gitignore"), ".claude/\n").unwrap();
        std::fs::write(root.join("CLAUDE.md"), "x").unwrap();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(root.join(".aida/config.toml"), "x").unwrap();
        std::fs::create_dir_all(root.join(".claude/skills")).unwrap();
        std::fs::write(root.join(".claude/skills/foo.md"), "x").unwrap();

        // Force the auto-commit branch (env override beats the TTY heuristic).
        std::env::set_var("AIDA_INIT_COMMIT_SCAFFOLD", "1");
        let committed = commit_init_scaffolding(root).unwrap();
        std::env::remove_var("AIDA_INIT_COMMIT_SCAFFOLD");

        // The remainder was committed → onboarding task is de-stranded.
        assert!(
            committed,
            "non-ignored scaffolding must commit even when one path is gitignored"
        );

        // The committed tree carries the non-ignored paths but NOT .claude.
        let tree = std::process::Command::new("git")
            .current_dir(root)
            .args(["ls-tree", "-r", "--name-only", "HEAD"])
            .output()
            .unwrap();
        let tracked = String::from_utf8_lossy(&tree.stdout);
        assert!(tracked.contains("CLAUDE.md"), "CLAUDE.md must be committed");
        assert!(
            tracked.contains(".aida/config.toml"),
            ".aida/config.toml must be committed"
        );
        assert!(
            !tracked.contains(".claude"),
            "the gitignored .claude path must NOT be committed (intent-respecting): {tracked}"
        );
    }

    /// Set up a temp git repo standing in for a fresh CLONE: an initial commit
    /// exists on `main`, and a fake `origin` remote points HEAD at it so HEAD ==
    /// origin/main at the start. Returns the repo root + the HEAD sha. The
    // scaffold files are written but NOT committed by the helper. trace:BUG-570
    fn setup_clone_like_repo(tmp: &TempDir) -> (std::path::PathBuf, String) {
        let root = tmp.path().to_path_buf();
        git_in(&root, &["init", "-q", "-b", "main"]);
        git_in(&root, &["config", "user.email", "t@t"]);
        git_in(&root, &["config", "user.name", "t"]);
        git_in(
            &root,
            &["commit", "--allow-empty", "-q", "-m", "upstream root"],
        );
        // A bare "origin" so origin/main resolves to the same commit (a clone
        // starts with HEAD == origin/main).
        let origin = tmp.path().join("origin.git");
        git_in(&root, &["init", "-q", "--bare", origin.to_str().unwrap()]);
        git_in(
            &root,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git_in(&root, &["push", "-q", "origin", "main"]);
        let head = git_head(&root);
        (root, head)
    }

    fn git_head(root: &std::path::Path) -> String {
        let out = std::process::Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Write the scaffold files a bootstrap-clone would land locally, then drive
    /// `complete_init_scaffolding`. Returns the HEAD sha afterward.
    fn run_complete_scaffolding(root: &std::path::Path, suppress: bool) -> String {
        // The Claude profile keeps the scaffold set small + deterministic.
        let store = aida_core::models::RequirementsStore::new();
        let db_path = root.join(".aida-store");
        complete_init_scaffolding(
            root,
            &store,
            "claude",
            false, // no_skills
            true,  // no_hooks — keep the scaffold set lean for the test
            false, // force
            db_path,
            "test store",
            false, // verbose
            suppress,
        )
        .unwrap();
        git_head(root)
    }

    /// BUG-570 criterion 1+5: a bootstrap-clone init (suppress=true) must NOT
    /// create a code-branch commit — HEAD stays exactly where origin/main is.
    /// The scaffold files are written (working tree dirty) but uncommitted.
    // trace:BUG-570 | ai:claude
    #[test]
    fn bootstrap_clone_init_leaves_head_unchanged() {
        let tmp = TempDir::new().unwrap();
        let (root, head_before) = setup_clone_like_repo(&tmp);

        let head_after = run_complete_scaffolding(&root, /* suppress */ true);

        assert_eq!(
            head_before, head_after,
            "bootstrap-clone init must not create a new code-branch commit"
        );
        // And it actually wrote local scaffold files (uncommitted).
        assert!(
            root.join("CLAUDE.md").exists(),
            "scaffold files should be written locally even when the commit is suppressed"
        );
        let status = std::process::Command::new("git")
            .current_dir(&root)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&status.stdout).trim().is_empty(),
            "the written scaffold files should leave the working tree dirty (uncommitted)"
        );
    }

    /// BUG-570 criterion 2: non-interactive (no TTY, the test harness) init on a
    /// bootstrap-clone must NOT auto-commit. Same HEAD-unchanged guarantee as the
    /// TTY case — suppression is independent of the TTY heuristic. The test
    /// process has no TTY, so this exercises exactly the dangerous silent path.
    // trace:BUG-570 | ai:claude
    #[test]
    fn bootstrap_clone_init_no_tty_does_not_autocommit() {
        // Belt-and-suspenders: even if some env tried to force auto-commit, the
        // bootstrap-clone suppression must win.
        std::env::remove_var("AIDA_INIT_COMMIT_SCAFFOLD");
        let tmp = TempDir::new().unwrap();
        let (root, head_before) = setup_clone_like_repo(&tmp);

        let head_after = run_complete_scaffolding(&root, /* suppress */ true);

        assert_eq!(
            head_before, head_after,
            "non-TTY bootstrap-clone init must not silently auto-commit a scaffold dump"
        );
    }

    /// BUG-570 criterion 3: a genuinely-new first-init (suppress=false) STILL
    /// commits its scaffolding — no regression. In the no-TTY test harness the
    /// auto-commit branch fires, so HEAD must advance past the root commit.
    // trace:BUG-570 | ai:claude
    #[test]
    fn genuinely_new_init_still_commits_scaffolding() {
        std::env::remove_var("AIDA_INIT_COMMIT_SCAFFOLD");
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        git_in(root, &["init", "-q", "-b", "main"]);
        git_in(root, &["config", "user.email", "t@t"]);
        git_in(root, &["config", "user.name", "t"]);
        git_in(root, &["commit", "--allow-empty", "-q", "-m", "root"]);
        let head_before = git_head(root);

        let head_after = run_complete_scaffolding(root, /* suppress */ false);

        assert_ne!(
            head_before, head_after,
            "genuinely-new first-init must still commit its scaffolding (no BUG-570 regression)"
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_init_distributed_worktree(
    force: bool,
    no_skills: bool,
    agent: &str,
    no_hooks: bool,
    verbose: bool,
    name: Option<&str>,
    git_init: bool,
    // BUG-570: forwarded to the bootstrap-clone path so `--commit-scaffold`
    // can opt into a deliberate scaffold commit on a clone. trace:BUG-570
    commit_scaffold: bool,
    // STORY-652: friendly node name for the auto-acquire at init time. None →
    // computed default (`<host>-<user>-<seq>`); at a TTY we prompt with the
    // default pre-filled. trace:STORY-652
    node_name: Option<&str>,
) -> Result<()> {
    use aida_core::git_ops;

    let cwd = std::env::current_dir()?;
    let aida_dir = cwd.join(".aida");
    let worktree_dir = ".aida-store";
    let branch_name = "aida-store";

    // STORY-552: complete the onboarding funnel at the FRONT. A new user in a
    // fresh folder who hasn't run `git init` shouldn't have to learn the recipe
    // and re-run init — that blocked first state is confusing. At a TTY, offer
    // to `git init` here; with --git-init, do it non-interactively. Otherwise
    // keep the clean bail+recipe (don't silently git-init in scripts — that's a
    // surprising side effect). The orphan-store machinery's "ensure at least one
    // commit" step below then creates HEAD if the freshly-init'd repo is empty,
    // and the BUG-446 workspace guard still runs AFTER this. trace:STORY-552 | ai:claude
    if !git_ops::is_git_repo(&cwd) {
        let at_tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
        let do_init = match git_init_decision(git_init, at_tty) {
            GitInitDecision::Yes => true,
            GitInitDecision::Bail => false,
            GitInitDecision::Prompt => prompt_yes_no(
                "No git repository here — initialize one with `git init`? [Y/n] ",
                true,
            )
            .unwrap_or(false),
        };

        if do_init {
            git_ops::init(&cwd)?;
            println!("  {} a git repository here", "Created".green());
        } else {
            anyhow::bail!(
                "Not a git repository. Run 'git init' first (or pass --git-init), or use --sibling for a separate repo."
            );
        }
    }

    // BUG-446: refuse to initialize over a workspace-of-projects. A fresh
    // `aida init` (no `.aida/config.toml` yet) whose directory contains nested,
    // non-submodule git repos would capture the whole tree as untracked entries
    // (nested repos becoming gitlinks) and root the orphan store at the
    // workspace level — every `aida` command from a future project subdirectory
    // would then climb UP to this workspace-wide store. Gate on not-yet-init'd
    // so re-running in a set-up project (handled below) keeps its own message;
    // bypass with --force. trace:BUG-446 | ai:claude
    if !force && !aida_dir.join("config.toml").exists() {
        let nested = unmanaged_nested_projects(&cwd);
        if !nested.is_empty() {
            let preview = nested
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let more = if nested.len() > 5 {
                format!(", … (+{} more)", nested.len() - 5)
            } else {
                String::new()
            };
            anyhow::bail!(
                "This directory looks like a workspace of projects, not a single project: \
                 it contains {} child {} ({}{}).\n\n\
                 `aida init` here would capture the entire tree and root the requirement \
                 store at the workspace level — every `aida` command from a project \
                 subdirectory would then operate on this workspace-wide store, and every \
                 child would inherit this scaffold's CLAUDE.md via ancestor lookup.\n\n\
                 Run `aida init` inside an actual project directory instead, \
                 or pass --force to initialize here anyway.",
                nested.len(),
                if nested.len() == 1 {
                    "project (git repo or AIDA project)"
                } else {
                    "projects (git repos or AIDA projects)"
                },
                preview,
                more,
            );
        }
    }

    // Post-clone detection (EPIC-1-052 Phase 4): if origin already has the
    // aida-store branch and the local worktree is missing, the user just
    // cloned an existing AIDA project. Bootstrap as a clone — fetch the
    // orphan, set up the worktree, run scaffolding, prompt for node id.
    // This wins over the "already initialized" check below because some
    // projects (notably AIDA itself) track .aida/config.toml in main.
    // trace:EPIC-1-052 Phase 4 | ai:claude
    let worktree_present = cwd.join(worktree_dir).exists();
    if !force && !worktree_present && git_ops::remote_branch_exists(&cwd, "origin", branch_name) {
        return handle_init_post_clone(
            &cwd,
            worktree_dir,
            branch_name,
            no_skills,
            agent,
            no_hooks,
            verbose,
            commit_scaffold,
            node_name,
        );
    }

    // Check if already initialized
    if aida_dir.join("config.toml").exists() && !force {
        // TASK-623: don't dead-end at "already initialized" when the store is
        // attached but this clone never acquired its own node id. That happens
        // when a read command auto-attached the worktree (TASK-621) and the
        // user then runs `aida init` to finish setup — or when a clone's
        // node-id step soft-failed (BUG-429). Route back through the post-clone
        // bootstrap, which is idempotent for the already-done parts (worktree
        // present → no-op, scaffolding force=false → skips existing) and runs
        // the node-id setup. Reliable now that node.toml is strictly per-clone
        // (BUG-430): a missing `.aida-store/.aida/node.toml` means no id yet.
        // trace:TASK-623 | ai:claude
        let node_configured = cwd
            .join(worktree_dir)
            .join(".aida")
            .join("node.toml")
            .exists();
        if worktree_present
            && !node_configured
            && git_ops::remote_branch_exists(&cwd, "origin", branch_name)
        {
            eprintln!(
                "{} store is attached but this clone has no node id yet — finishing setup.",
                "Note:".dimmed()
            );
            return handle_init_post_clone(
                &cwd,
                worktree_dir,
                branch_name,
                no_skills,
                agent,
                no_hooks,
                verbose,
                commit_scaffold,
                node_name,
            );
        }
        eprintln!(
            "{} AIDA distributed mode is already initialized (.aida/config.toml exists).",
            "!".yellow()
        );
        eprintln!("  Use {} to reinitialize.", "--force".bold());
        eprintln!("  To refresh just the scaffolding (CLAUDE.md, .claude/skills/, hooks),");
        eprintln!("  use `aida scaffold apply --force` instead — preserves your store.");
        return Ok(());
    }

    // --force on a populated store would silently wipe N requirements and reseed
    // an empty store with just META prompts. Surface that loudly: count what
    // would be lost and require the user to type "reset" to acknowledge.
    if force {
        let existing_store = cwd.join(worktree_dir);
        if let Some(count) = count_requirements_in_store(&existing_store) {
            if count > 0 && !confirm_destructive_reset(count, &existing_store)? {
                return Ok(());
            }
        }
    }

    println!("{}", "Setting up AIDA in this project…".bold());
    println!();

    // Ensure there's at least one commit on main (worktree requires it)
    let has_commits = git_ops::head_sha(&cwd).is_ok();
    if !has_commits {
        // Create an initial commit so worktree can be added
        std::fs::write(cwd.join(".gitkeep"), "")?;
        git_ops::add(&cwd, &[".gitkeep"])?;

        let git_name =
            git_ops::git_config_get("user.name").unwrap_or_else(|_| "AIDA User".to_string());
        let git_email =
            git_ops::git_config_get("user.email").unwrap_or_else(|_| "aida@localhost".to_string());
        git_ops::configure_user(&cwd, &git_name, &git_email)?;
        git_ops::commit(&cwd, "chore: initial commit")?;
    }

    // Create orphan branch + worktree
    let store_path = git_ops::create_store_worktree(&cwd, worktree_dir, branch_name)?;
    // Setup-detail (orphan branch / node id / forge) is plumbing a newcomer
    // doesn't need to see — surface it only under --verbose. trace:TASK-725
    if verbose {
        println!(
            "  {} orphan branch '{}' with worktree at {}",
            "Created".green(),
            branch_name,
            worktree_dir
        );
    }

    // Configure git user in worktree
    let git_name = git_ops::git_config_get("user.name").unwrap_or_else(|_| "AIDA User".to_string());
    let git_email =
        git_ops::git_config_get("user.email").unwrap_or_else(|_| "aida@localhost".to_string());
    git_ops::configure_user(&store_path, &git_name, &git_email)?;

    // Initialize the git backend
    let backend = aida_core::GitBackend::new(&store_path)?;
    let mut store = aida_core::models::RequirementsStore::new();
    // Project name: explicit --name wins; otherwise default to the cwd
    // basename ("/home/joe/projects/tzconv" → "tzconv"). trace:BUG-25
    let project_name = name
        .map(|s| s.to_string())
        .or_else(|| {
            cwd.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default();
    if !project_name.is_empty() {
        store.name = project_name.clone();
        store.title = project_name;
    }
    seed_meta_requirements(&mut store)?;
    backend.save(&store)?;
    if verbose {
        println!(
            "  {} {}",
            "Created".green(),
            format!("{}/metadata.yaml", worktree_dir).white().bold()
        );
    }

    // Create initial commit on the orphan branch
    git_ops::add(&store_path, &["metadata.yaml"])?;
    std::fs::create_dir_all(store_path.join("objects"))?;
    std::fs::write(store_path.join("objects/.gitkeep"), "")?;
    git_ops::add(&store_path, &["objects/.gitkeep"])?;
    git_ops::add(&store_path, &["objects"])?;
    git_ops::commit(&store_path, "chore: initialize AIDA distributed store")?;

    // Auto-push to origin/<branch_name> if origin exists, so subsequent
    // node-acquire / db-sync work without a manual `git push -u`. Skipped
    // silently when there's no origin (single-machine projects) or when
    // the push fails (offline, auth, etc.) — failure isn't fatal here.
    // trace:BUG-23 trace:TASK-421 | ai:claude
    //
    // BUG-559 root-cause prevention (TASK-844): the orphan-store push below is
    // the FIRST push to a fresh origin. A forge that adopts the first-pushed
    // branch as its default (GitLab's push-to-create) would therefore make the
    // orphan `aida-store` the project default — breaking fresh-clone checkout
    // (the clone checks out the internal YAML store as code). Prevent that two
    // ways, both gated on the origin being EMPTY (a real push-to-create) so we
    // never change behavior for an existing repo:
    //   (a) push the CODE branch (main/master) FIRST, before the orphan store,
    //       so the forge adopts main as default. Guardrail: only when the code
    //       branch exists and has commits (it always does here — init creates an
    //       initial commit above — but the pure `decide_init_push_plan` keeps the
    //       conservative-fallback contract explicit and testable).
    //   (b) after the first real push, explicitly set the remote default branch
    //       to the code branch via the forge API (`gh repo edit` / `glab api`),
    //       belt-and-suspenders for forges/CLIs that don't honor push order.
    // GitHub already defaults to main via repo-create, so (a) is harmless there;
    // (b) is a no-op reassert. Live end-to-end verification against GitHub +
    // GitLab is a manual maintainer step. trace:TASK-844 trace:BUG-559 | ai:claude
    if git_ops::has_remote(&cwd, "origin") {
        // The code branch is the main-repo's current branch (orphan-worktree
        // creation didn't switch it). Push-to-create = origin has no heads yet.
        let code_branch = git_ops::current_branch(&cwd).ok();
        let origin_empty = git_ops::remote_has_no_heads(&cwd, "origin");
        // Init guarantees the code branch carries at least an initial commit by
        // this point, so HEAD resolves; the pure decider still encodes the
        // "must have commits" guardrail explicitly. trace:TASK-844
        let code_branch_has_commits = git_ops::head_sha(&cwd).is_ok();
        let plan = decide_init_push_plan(code_branch.as_deref(), code_branch_has_commits);

        // (a) On an empty origin, push the code branch first so the forge adopts
        //     it as default rather than the orphan store.
        if origin_empty {
            if let InitPushPlan::CodeBranchFirst(cb) = &plan {
                let out = std::process::Command::new("git")
                    .arg("-C")
                    .arg(&cwd)
                    .args(["push", "-u", "origin", cb])
                    .output();
                match out {
                    Ok(o) if o.status.success() => {
                        println!("  {} pushed code branch to origin/{}", "Done".green(), cb);
                    }
                    _ => {
                        eprintln!(
                            "  {} could not push code branch to origin/{} \
                             (run `git push -u origin {}` later)",
                            "Note:".dimmed(),
                            cb,
                            cb
                        );
                    }
                }
            }
        }

        // The orphan-store push (unchanged behavior).
        let push_result = std::process::Command::new("git")
            .arg("-C")
            .arg(&store_path)
            .args(["push", "-u", "origin", branch_name])
            .output();
        match push_result {
            Ok(out) if out.status.success() => {
                println!(
                    "  {} pushed orphan branch to origin/{}",
                    "Done".green(),
                    branch_name
                );
            }
            _ => {
                eprintln!(
                    "  {} could not push orphan branch to origin/{} \
                     (run `git -C {} push -u origin {}` later)",
                    "Note:".dimmed(),
                    branch_name,
                    worktree_dir,
                    branch_name
                );
            }
        }

        // (b) Explicitly assert the forge default branch = the code branch, so
        //     even a forge/CLI that ignores push order ends up correct. Only on
        //     a push-to-create (empty origin) and only when we know the code
        //     branch; best-effort (a missing forge CLI / auth just no-ops).
        if origin_empty {
            if let InitPushPlan::CodeBranchFirst(cb) = &plan {
                let kind = crate::forge::resolve_forge_kind(&cwd);
                let project_ref = crate::forge::origin_url(&cwd)
                    .as_deref()
                    .and_then(crate::forge::project_path_of);
                if let Some(argv) = kind.set_default_branch_cmd(cb, project_ref.as_deref()) {
                    if let Some((prog, args)) = argv.split_first() {
                        let out = std::process::Command::new(prog).args(args).output();
                        if let Ok(o) = out {
                            if o.status.success() {
                                println!("  {} set forge default branch to {}", "Done".green(), cb);
                            }
                        }
                    }
                }
            }
        }
    }

    // Auto-acquire a node id for this clone so nodes.toml has the entry
    // from day one (instead of relying on the implicit-node-1 fallback).
    // Runs whether or not origin exists: when there's no remote, the
    // registration commit lives on the local orphan branch and gets
    // uploaded by the next `aida push`. Without this the solo user can't
    // get their preferred node id (e.g. JM from prefs) without first
    // adding a remote — exactly the kind of friction kernel should hide.
    // trace:EPIC-9 Story 1, BUG-40 | ai:claude
    let has_origin = git_ops::has_remote(&cwd, "origin");
    {
        let hn = hostname();
        // Prefer email from `git config user.email`; fall back to
        // `~/.aida/preferences.toml`. trace:STORY-44 | ai:claude
        let prefs = aida_core::UserPreferences::load().unwrap_or_default();
        let email = git_ops::git_config_get("user.email")
            .ok()
            .or_else(|| prefs.email.clone());

        // STORY-44: try the preferred id first; on collision the kernel's
        // suggest_free_node_id helper picks the next free `<pref><N>` and
        // we auto-accept (init is non-interactive and the alternative is a
        // bad first impression). Without a preference, fall back to the
        // pre-EPIC-9 sequential numeric path.
        let requested_id: Option<String> = match prefs.preferred_node_id.as_deref() {
            Some(pref) => match git_ops::suggest_free_node_id(&store_path, pref) {
                Ok(git_ops::NodeIdProbe::Free) => Some(pref.to_string()),
                Ok(git_ops::NodeIdProbe::Taken { suggested }) => {
                    println!(
                        "  {} preferred node id '{}' is already taken — using '{}' instead",
                        "Note:".dimmed(),
                        pref,
                        suggested
                    );
                    Some(suggested)
                }
                Err(_) => None,
            },
            None => None,
        };

        // STORY-652: capture the owner $USER string + resolve the friendly
        // node name. The seq used for the default-name preview is the
        // requested id when known, else the registry's predicted next id; if
        // the user accepts the default we pass None so core stamps the name
        // with the actually-assigned id. trace:STORY-652 | ai:claude
        let owner_user = current_user_id(None);
        let predicted_seq = requested_id.clone().unwrap_or_else(|| {
            aida_core::node::NodeRegistry::load(&store_path.join("registry").join("nodes.toml"))
                .map(|r| r.next_node_id())
                .unwrap_or_else(|_| "1".to_string())
        });
        let resolved_name = resolve_node_name(node_name, &hn, &owner_user, &predicted_seq)?;
        let default_name = aida_core::node::default_node_name(&hn, &owner_user, &predicted_seq);
        let identity = git_ops::NodeIdentity {
            name: if resolved_name == default_name {
                None
            } else {
                Some(resolved_name)
            },
            user: Some(owner_user.clone()),
        };
        match git_ops::register_node_full_identity(
            &store_path,
            requested_id,
            1,
            &hn,
            email.clone(),
            identity,
        ) {
            Ok(new_id) => {
                let suffix = if has_origin {
                    ""
                } else {
                    " (local; will sync on next `aida push`)"
                };
                if verbose {
                    println!(
                        "  {} acquired node id {} (hostname={}, email={}){}",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        new_id,
                        hn,
                        email.as_deref().unwrap_or("-"),
                        suffix
                    );
                }
                // FR-271: at init time, force the new-project default
                // (Global) explicitly. Reading config.toml here would
                // return PerType because we haven't written the config
                // yet (it's written further down in the init flow).
                if let Ok(blocks) = auto_allocate_initial_blocks_with_scope(
                    &store_path,
                    &new_id,
                    &hn,
                    email.as_deref(),
                    aida_core::IdCounterScope::Global,
                ) {
                    if !blocks.is_empty() && verbose {
                        println!(
                            "  {} auto-allocated {} initial block{}",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            blocks.len(),
                            if blocks.len() == 1 { "" } else { "s" }
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "  {} could not auto-acquire node id: {}. Run `aida node acquire` later.",
                    "Note:".dimmed(),
                    e
                );
            }
        }
    }

    // Add .aida-store and runtime shims to .gitignore on main branch.
    // trace:BUG-71 | ai:claude
    add_aida_gitignore_entries(&cwd, worktree_dir)?;

    // Create .aida/config.toml
    std::fs::create_dir_all(&aida_dir)?;
    let config_content = format!(
        "# AIDA distributed mode configuration\n\
         [deployment]\n\
         mode = \"distributed\"\n\
         store_path = \"{}\"\n\
         store_type = \"worktree\"\n\
         branch = \"{}\"\n\
         \n\
         [store.sync]\n\
         # Auto-push store commits after local writes. Values: manual,\n\
         # session-end, per-write, periodic. `periodic` is reserved until\n\
         # aida-worker (EPIC-30) ships.\n\
         auto_push = \"manual\"\n\
         \n\
         # trace:EPIC-1-052 Phase 2 | ai:claude\n\
         # How `aida add` chooses between agreed-id blocks and node-aware ids:\n\
         #   node-aware-only      — never use blocks; always FR-<NODE>-<SEQ>\n\
         #   blocks-then-fallback — try block first; fall through silently (default)\n\
         #   blocks-only          — error if no block is allocated for the type\n\
         #\n\
         # counter_scope (FR-271):\n\
         #   global               — single counter shared across all types (default for new projects)\n\
         #                          → FR-1, BUG-2, EPIC-3, ... ids globally unique by number\n\
         #   per-type             — separate counter per type prefix (legacy default)\n\
         #                          → FR-1, BUG-1, EPIC-1, ... each type starts fresh\n\
         [id_format]\n\
         policy = \"blocks-then-fallback\"\n\
         counter_scope = \"global\"\n\
         \n\
         # trace:TASK-281 | ai:claude\n\
         # Auto-claim a fresh block when aggregate remaining IDs drop below\n\
         # threshold. On by default (threshold 20, size 100). Opt out with:\n\
         #   [block_allocation]\n\
         #   auto_claim = false\n\
         # Per-type override (e.g. larger BUG blocks):\n\
         #   [block_allocation.bug]\n\
         #   auto_claim_threshold = 50\n\
         #   auto_claim_size = 200\n",
        worktree_dir, branch_name
    );
    // EPIC-35: scaffold the [forge] section with the auto-detected provider.
    let config_content = config_content + &forge::init_forge_config_section(&cwd);
    // TASK-304: scaffold the [ultraplan] cadence block (mode = on-demand).
    let config_content = config_content + init_ultraplan_config_section();
    // Commented [intake] example block (defaults are safe; discoverability only).
    // trace:TASK-760 | ai:claude
    let config_content = config_content + init_intake_config_section();
    // STORY-714/TASK-985: warm-pool ON by default (escape hatches documented).
    let config_content = config_content + init_worktree_pool_config_section();
    // STORY-760: commented [store.sync] mirror_remotes fan-out stub.
    let config_content = config_content + init_store_mirror_config_section();
    std::fs::write(aida_dir.join("config.toml"), &config_content)?;

    // STORY-511: surface the auto-detected forge so the operator sees the
    // inference instead of having to read .aida/config.toml. EPIC-35 init UX.
    if verbose {
        let (_, msg) = forge::init_forge_detection_message(&cwd);
        println!(
            "  {} {}",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            msg
        );
    }

    // Create docs/plans/ for plan archive (per CLAUDE.md convention).
    std::fs::create_dir_all(cwd.join("docs/plans"))?;
    ensure_plan_template_scaffold(&cwd.join("docs/plans"), force)?;

    // Run the shared workflow scaffolding (skills, hooks, mcp, codex).
    let storage_label = format!(
        "{}{}your specs live here (git-tracked, synced with your code)",
        worktree_dir.white().bold(),
        " ".repeat(20),
    );
    complete_init_scaffolding(
        &cwd,
        &store,
        agent,
        no_skills,
        no_hooks,
        force,
        std::path::PathBuf::from(worktree_dir),
        &storage_label,
        verbose,
        // Genuinely-new first-init: commit the scaffolding as before (BUG-570
        // suppression applies only to the bootstrap-clone path).
        false,
    )?;

    // Sharing + teammate onboarding are real, but premature for a solo
    // newcomer with no remote yet — surface them under --verbose (and they're
    // re-surfaced contextually when there IS an origin). trace:TASK-725
    if verbose {
        println!();
        println!("  {}:", "Push code + store together".bold());
        println!(
            "    {}                        push your branch and the spec store in one go",
            "aida push".cyan()
        );
        println!();
        println!("  {}:", "Onboard a teammate".bold());
        println!("    {}    they clone normally", "git clone <repo>".cyan());
        println!(
            "    {}            then `aida init` attaches the shared specs automatically",
            "aida init".cyan()
        );
    }
    println!();

    // STORY-763: on a codex-first machine (the uniform `[agents] vendor`
    // knob resolves to codex), also write the Codex custom-prompt set so
    // /aida-... works in Codex sessions from the first init. Idempotent and
    // conservative (skip-existing), best-effort — a failure here must not
    // fail init.
    maybe_scaffold_codex_prompts_on_init(std::path::Path::new("."));

    Ok(())
}

/// STORY-763: init-time hook — when the resolved default vendor is codex,
/// write the Codex custom prompts to ~/.codex/prompts (skip-existing, never
/// forced). Quiet no-op for claude-default machines. Best-effort: warns
/// instead of failing init.
fn maybe_scaffold_codex_prompts_on_init(project_root: &std::path::Path) {
    if aida_core::agents_config::resolve_default_vendor(project_root).as_deref() != Some("codex") {
        return;
    }
    let Some(dest) = dirs::home_dir().map(|h| h.join(".codex").join("prompts")) else {
        return;
    };
    match aida_core::scaffolding::codex_prompts::scaffold_codex_prompts(&dest, false) {
        Ok(outcome) if !outcome.written.is_empty() => {
            println!(
                "  {} codex-first machine: wrote {} Codex custom prompt(s) to {} (/aida-... now works in Codex sessions)",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                outcome.written.len(),
                dest.display()
            );
        }
        Ok(_) => {} // everything already present — quiet.
        Err(e) => {
            eprintln!(
                "  {} codex prompt scaffold skipped ({e}) — run `aida scaffold codex-prompts` by hand",
                "Warning:".yellow()
            );
        }
    }
}

/// Bootstrap an AIDA clone: the user just `git clone`d a repo whose origin
/// already has the `aida-store` orphan branch, and they're running `aida init`
/// to set the project up locally. We fetch the orphan, attach a worktree,
/// run scaffolding, and prompt for node-id acquisition.
// trace:EPIC-1-052 Phase 4 | ai:claude
#[allow(clippy::too_many_arguments)]
fn handle_init_post_clone(
    cwd: &std::path::Path,
    worktree_dir: &str,
    branch_name: &str,
    no_skills: bool,
    agent: &str,
    no_hooks: bool,
    verbose: bool,
    // BUG-570: opt-in `--commit-scaffold` to deliberately commit the locally
    // written scaffold files on a clone. Default false → leave them uncommitted
    // so a clone never pushes a scaffold dump to the shared default branch.
    // trace:BUG-570 | ai:claude
    commit_scaffold: bool,
    // STORY-652: friendly node name for the clone's node acquisition. None →
    // computed default; prompted at a TTY. trace:STORY-652
    node_name: Option<&str>,
) -> Result<()> {
    use aida_core::git_ops;

    println!(
        "{} Detected existing AIDA store on origin/{} — bootstrapping clone...",
        "".cyan().bold(),
        branch_name
    );

    // Fetch + create local tracking branch for the orphan — UNLESS the
    // worktree is already attached. A read command may have auto-attached it
    // (TASK-621), which already created the local `aida-store` branch and
    // checked it out; re-fetching `branch:branch` into a checked-out branch
    // errors ("refusing to fetch into branch ... checked out"), and
    // create_store_worktree would no-op anyway. So when `aida init` is
    // re-run on an auto-attached clone to finish node-id setup (TASK-623),
    // skip straight past the worktree steps. trace:TASK-623 | ai:claude
    let store_path = if cwd.join(worktree_dir).exists() {
        cwd.join(worktree_dir)
    } else {
        git_ops::fetch_branch_into_local(cwd, "origin", branch_name)?;
        println!(
            "  {} fetched origin/{} into local {}",
            "Done".green(),
            branch_name,
            branch_name
        );
        let sp = git_ops::create_store_worktree(cwd, worktree_dir, branch_name)?;
        println!(
            "  {} worktree at {} → {}",
            "Done".green(),
            worktree_dir,
            branch_name
        );
        sp
    };

    // Configure git user in the worktree (so future commits attribute correctly)
    let git_name = git_ops::git_config_get("user.name").unwrap_or_else(|_| "AIDA User".to_string());
    let git_email =
        git_ops::git_config_get("user.email").unwrap_or_else(|_| "aida@localhost".to_string());
    git_ops::configure_user(&store_path, &git_name, &git_email)?;

    // Add .aida-store/ and runtime shims to root .gitignore (idempotent).
    // trace:BUG-71 | ai:claude
    if add_aida_gitignore_entries(cwd, worktree_dir)? {
        println!(
            "  {} updated {}",
            "Done".green(),
            ".gitignore".white().bold()
        );
    }

    // Write .aida/config.toml — same contents as a fresh init
    let aida_dir = cwd.join(".aida");
    std::fs::create_dir_all(&aida_dir)?;
    let config_content = format!(
        "# AIDA distributed mode configuration\n\
         [deployment]\n\
         mode = \"distributed\"\n\
         store_path = \"{}\"\n\
         store_type = \"worktree\"\n\
         branch = \"{}\"\n\
         \n\
         [store.sync]\n\
         # Auto-push store commits after local writes. Values: manual,\n\
         # session-end, per-write, periodic. `periodic` is reserved until\n\
         # aida-worker (EPIC-30) ships.\n\
         auto_push = \"manual\"\n\
         \n\
         # trace:EPIC-1-052 Phase 2 | ai:claude\n\
         # How `aida add` chooses between agreed-id blocks and node-aware ids:\n\
         #   node-aware-only      — never use blocks; always FR-<NODE>-<SEQ>\n\
         #   blocks-then-fallback — try block first; fall through silently (default)\n\
         #   blocks-only          — error if no block is allocated for the type\n\
         #\n\
         # counter_scope (FR-271):\n\
         #   global               — single counter shared across all types (default for new projects)\n\
         #                          → FR-1, BUG-2, EPIC-3, ... ids globally unique by number\n\
         #   per-type             — separate counter per type prefix (legacy default)\n\
         #                          → FR-1, BUG-1, EPIC-1, ... each type starts fresh\n\
         [id_format]\n\
         policy = \"blocks-then-fallback\"\n\
         counter_scope = \"global\"\n\
         \n\
         # trace:TASK-281 | ai:claude\n\
         # Auto-claim a fresh block when aggregate remaining IDs drop below\n\
         # threshold. On by default (threshold 20, size 100). Opt out with:\n\
         #   [block_allocation]\n\
         #   auto_claim = false\n\
         # Per-type override (e.g. larger BUG blocks):\n\
         #   [block_allocation.bug]\n\
         #   auto_claim_threshold = 50\n\
         #   auto_claim_size = 200\n",
        worktree_dir, branch_name
    );
    // EPIC-35: scaffold the [forge] section with the auto-detected provider.
    let config_content = config_content + &forge::init_forge_config_section(cwd);
    // TASK-304: scaffold the [ultraplan] cadence block (mode = on-demand).
    let config_content = config_content + init_ultraplan_config_section();
    // Commented [intake] example block (defaults are safe; discoverability only).
    // trace:TASK-760 | ai:claude
    let config_content = config_content + init_intake_config_section();
    // STORY-714/TASK-985: warm-pool ON by default (escape hatches documented).
    let config_content = config_content + init_worktree_pool_config_section();
    // STORY-760: commented [store.sync] mirror_remotes fan-out stub.
    let config_content = config_content + init_store_mirror_config_section();
    std::fs::write(aida_dir.join("config.toml"), &config_content)?;
    println!(
        "  {} {}",
        "Done".green(),
        ".aida/config.toml".white().bold()
    );
    // STORY-511: surface the auto-detected forge (EPIC-35 init UX).
    {
        let (_, msg) = forge::init_forge_detection_message(cwd);
        println!("  {} {}", "Done".green(), msg);
    }

    // docs/plans/ for plan archive (post-clone attach: never overwrite).
    std::fs::create_dir_all(cwd.join("docs/plans"))?;
    ensure_plan_template_scaffold(&cwd.join("docs/plans"), false)?;

    // Run scaffolding (CLAUDE.md, .claude/, hooks, etc.)
    // Load the store via GitBackend just for scaffolding metadata.
    let backend = aida_core::GitBackend::new(&store_path)?;
    let store = backend
        .load()
        .unwrap_or_else(|_| aida_core::models::RequirementsStore::new());
    let storage_label = format!(
        "{}{}your specs live here (git-tracked, synced with your code)",
        worktree_dir.white().bold(),
        " ".repeat(20),
    );
    complete_init_scaffolding(
        cwd,
        &store,
        agent,
        no_skills,
        no_hooks,
        false, // force
        std::path::PathBuf::from(worktree_dir),
        &storage_label,
        verbose,
        // BUG-570: bootstrap-clone → suppress the scaffold commit by default.
        // Only `--commit-scaffold` opts into a deliberate code-branch commit.
        // trace:BUG-570 | ai:claude
        !commit_scaffold,
    )?;

    // Prompt the user to acquire a node id. Auto-allocate happens inside
    // `aida node acquire` per Phase 3; we just wire up the same code path.
    println!();
    println!("{}", "Node identity setup".cyan().bold());
    println!("  This clone needs a unique node id to issue requirement IDs without colliding");
    println!("  with other clones. Acquire one now? (Recommended.)");

    let acquire_now = prompt_yes_no("Acquire a node id for this clone? [Y/n] ", true)?;
    if !acquire_now {
        println!();
        println!(
            "  {} You can run {} later — until then, this clone will share node id 1's namespace.",
            "Note:".yellow().bold(),
            "aida node acquire".cyan()
        );
        return Ok(());
    }

    let hn = hostname();
    // Prefer email from `git config user.email`; fall back to user prefs.
    // trace:STORY-44 | ai:claude
    let prefs = aida_core::UserPreferences::load().unwrap_or_default();
    let email = git_ops::git_config_get("user.email")
        .ok()
        .or_else(|| prefs.email.clone());
    let requested_id: Option<String> = match prefs.preferred_node_id.as_deref() {
        Some(pref) => match git_ops::suggest_free_node_id(&store_path, pref) {
            Ok(git_ops::NodeIdProbe::Free) => Some(pref.to_string()),
            Ok(git_ops::NodeIdProbe::Taken { suggested }) => {
                // Leading newline because this often fires immediately after
                // the user types `y` to the "Acquire? [Y/n] " prompt — without
                // it the Note butts up against the prompt's trailing space.
                println!(
                    "\n  {} preferred node id '{}' is taken — using '{}' instead",
                    "Note:".dimmed(),
                    pref,
                    suggested
                );
                Some(suggested)
            }
            Err(_) => None,
        },
        None => None,
    };
    let id_label = match &requested_id {
        Some(id) => format!("id={}", id),
        None => "next available id".to_string(),
    };
    println!();
    println!(
        "Acquiring node ({}; hostname={}, email={})...",
        id_label,
        hn,
        email.as_deref().unwrap_or("-")
    );
    // STORY-652: owner $USER + friendly node name. trace:STORY-652
    let owner_user = current_user_id(None);
    let predicted_seq = requested_id.clone().unwrap_or_else(|| {
        aida_core::node::NodeRegistry::load(&store_path.join("registry").join("nodes.toml"))
            .map(|r| r.next_node_id())
            .unwrap_or_else(|_| "1".to_string())
    });
    let resolved_name = resolve_node_name(node_name, &hn, &owner_user, &predicted_seq)?;
    let default_name = aida_core::node::default_node_name(&hn, &owner_user, &predicted_seq);
    let identity = git_ops::NodeIdentity {
        name: if resolved_name == default_name {
            None
        } else {
            Some(resolved_name)
        },
        user: Some(owner_user.clone()),
    };
    let new_id = match git_ops::register_node_full_identity(
        &store_path,
        requested_id,
        1, // user_id placeholder — see Phase 1 commit message
        &hn,
        email.clone(),
        identity,
    ) {
        Ok(id) => {
            println!(
                "  {} Acquired node id {} for this clone.",
                "".green().bold(),
                id
            );
            id
        }
        // BUG-429: by here the store worktree is already attached + scaffolded
        // and the onboarding task is enqueued — only node-id acquisition
        // failed (transient registry contention, or an unreachable/rejecting
        // remote). Don't abort the whole clone-init as a hard error with a
        // bare "Error:" that reads as "nothing worked": the clone is usable
        // for reads right now, and only collision-free ID issuance for WRITES
        // needs the node id. Warn, point at the retry, and return Ok —
        // consistent with the auto-acquire path's soft-handling above.
        // trace:BUG-429 | ai:claude
        Err(e) => {
            eprintln!();
            eprintln!(
                "  {} Store attached, but node-id acquisition failed: {}",
                "Warning:".yellow().bold(),
                e
            );
            eprintln!(
                "  The clone is ready to read (list / show / findings / queue). Before issuing"
            );
            eprintln!(
                "  new IDs (`aida add`), run `{}` to claim a collision-free node id.",
                "aida node acquire".cyan()
            );
            eprintln!();
            eprintln!(
                "{} AIDA clone bootstrap complete (node-id pending).",
                "".green().bold()
            );
            return Ok(());
        }
    };

    // Auto-allocate initial blocks for the common types (Phase 3).
    // trace:FR-1-073 | ai:claude
    match auto_allocate_initial_blocks(&store_path, &new_id, &hn, email.as_deref()) {
        Ok(blocks) if !blocks.is_empty() => {
            println!(
                "  Auto-allocated {} block{}: {}",
                blocks.len(),
                if blocks.len() == 1 { "" } else { "s" },
                blocks.join(", ")
            );
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!(
                "  {} Could not auto-allocate initial blocks: {}. \
                 Run `aida db block claim --type <T> --size 100` per type to retry.",
                "Warning:".yellow().bold(),
                e
            );
        }
    }

    println!();
    println!("{} AIDA clone bootstrap complete.", "".green().bold());
    Ok(())
}

/// Initialize distributed mode using a sibling repo.
/// For multi-repo workspaces where multiple code repos share one store.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_init_distributed_sibling(
    registry_remote: Option<&str>,
    force: bool,
    attach: bool,
    store_path_arg: Option<&str>,
    no_skills: bool,
    agent: &str,
    no_hooks: bool,
    verbose: bool,
    name: Option<&str>,
) -> Result<()> {
    use aida_core::git_ops;

    let cwd = std::env::current_dir()?;
    let aida_dir = cwd.join(".aida");
    // BUG-608/STORY-676: the store is a separate repo at an explicit relative (or
    // absolute) path. `--sibling` defaults it to `../aida-store` — a TRUE SIBLING
    // of the code repo (not nested: a nested store can't be reached by a second
    // repo). `store_rel` is what lands in config.toml (resolved relative to the
    // project root, == cwd here); `store_dir` is its absolute form for git ops.
    // trace:BUG-608 trace:STORY-676 | ai:claude
    let store_rel = store_path_arg.unwrap_or("../aida-store");
    let store_dir = {
        let p = std::path::Path::new(store_rel);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            cwd.join(p)
        }
    };

    // Check if already initialized
    if aida_dir.join("node.toml").exists() && !force {
        eprintln!(
            "{} AIDA distributed mode is already initialized (.aida/node.toml exists).",
            "!".yellow()
        );
        eprintln!("  Use {} to reinitialize.", "--force".bold());
        eprintln!("  To refresh just the scaffolding (CLAUDE.md, .claude/skills/, hooks),");
        eprintln!("  use `aida scaffold apply --force` instead — preserves your store.");
        return Ok(());
    }

    // --force on a populated store would silently wipe N requirements. Same
    // guard as the worktree path: count + require typed confirmation.
    if force {
        if let Some(count) = count_requirements_in_store(&store_dir) {
            if count > 0 && !confirm_destructive_reset(count, &store_dir)? {
                return Ok(());
            }
        }
    }

    // BUG-610 / STORY-674: decide create-vs-attach-vs-refuse for an existing
    // populated store. The fresh-init path below re-seeds META and
    // `backend.save()`s a new store, which DELETES whatever was there (silent
    // data loss, verified: a second repo's init wiped the first's spec).
    //   --force   → deliberate wipe (guarded above)
    //   --attach  → JOIN the existing shared store (multi-repo): config + cache
    //               only, store untouched. The shared dispenser serializes id
    //               allocation across repos, so no separate node id is needed.
    //   neither   → refuse rather than destroy.
    // trace:BUG-610 trace:STORY-674 | ai:claude
    let existing_count = count_requirements_in_store(&store_dir).unwrap_or(0);
    let mut force = force;
    let mut attaching = attach && existing_count > 0 && !force;
    if existing_count > 0 && !force && !attach {
        let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
        if interactive {
            // STORY-675: a store already exists at the target path — instead of a
            // flat refusal, OFFER to join it (the multi-repo case is the common
            // one). Join routes through the STORY-674 attach path; "create new"
            // requires the same typed confirmation as --force. trace:STORY-675
            eprintln!(
                "{} A store already exists at {} with {} requirement(s).",
                "!".yellow(),
                store_dir.display(),
                existing_count
            );
            eprintln!("  Almost certainly a shared store another repo created. What now?");
            eprintln!(
                "  {} [{}] Join it — attach this repo to the existing store (recommended)",
                crate::glyph(crate::glyphs::Glyph::Bullet),
                "j".green().bold()
            );
            eprintln!(
                "  {} [{}] Create new — WIPE and re-initialize ({} requirement(s) lost)",
                crate::glyph(crate::glyphs::Glyph::Bullet),
                "c".red().bold(),
                existing_count
            );
            eprintln!(
                "  {} [{}] Abort (default)",
                crate::glyph(crate::glyphs::Glyph::Bullet),
                "a".bold()
            );
            eprint!("  Choose [j/c/a]: ");
            use std::io::Write;
            let _ = std::io::stderr().flush();
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            match input.trim().to_lowercase().as_str() {
                "j" | "join" => attaching = true,
                "c" | "create" | "new" => {
                    if !confirm_destructive_reset(existing_count, &store_dir)? {
                        return Ok(());
                    }
                    force = true;
                }
                _ => {
                    eprintln!("Aborted.");
                    return Ok(());
                }
            }
        } else {
            // Non-TTY: never silently overwrite. Refuse with the actionable
            // flags (BUG-610) and abort (non-zero exit, no post-setup).
            eprintln!(
                "{} A store already exists at {} with {} requirement(s).",
                "!".yellow(),
                store_dir.display(),
                existing_count
            );
            eprintln!("  Re-initializing would DELETE them — this is almost certainly a shared");
            eprintln!("  sibling store another repo created. Refusing to overwrite it.");
            eprintln!(
                "  {} To ATTACH this repo to the existing store (multi-repo): re-run with {}.",
                crate::glyph(crate::glyphs::Glyph::Bullet),
                "--attach".bold()
            );
            eprintln!(
                "  {} To WIPE and re-initialize the store: re-run with {}.",
                crate::glyph(crate::glyphs::Glyph::Bullet),
                "--force".bold()
            );
            anyhow::bail!(
                "refused to re-initialize the existing store at {} (pass --attach to join, or --force to wipe)",
                store_dir.display()
            );
        }
    }

    if attaching {
        println!(
            "{}",
            format!("Attaching to existing store at {} ...", store_dir.display()).bold()
        );
        println!();
    } else {
        println!("{}", "Initializing AIDA in distributed mode...".bold());
        println!();

        // Create the local git-backed store directory
        if !store_dir.exists() {
            std::fs::create_dir_all(&store_dir)?;
        }

        // Initialize git repo if not already
        if !git_ops::is_git_repo(&store_dir) {
            git_ops::init(&store_dir)?;
            println!(
                "  {} git repository in {}",
                "Created".green(),
                format!("{store_rel}/").white().bold()
            );
        }
    }

    // STORY-674: attach must NOT touch the shared store. It builds only a
    // minimal in-memory store (project name → scaffolding/CLAUDE.md title) and
    // skips the whole store-write path (configure/remote/seed/save/commit/
    // register). Create builds+seeds+saves a fresh store. Both yield `store`
    // for the shared scaffolding below. trace:STORY-674 | ai:claude
    let store = if attaching {
        let mut s = aida_core::models::RequirementsStore::new();
        let pn = name.map(|s| s.to_string()).or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        });
        if let Some(pn) = pn {
            if !pn.is_empty() {
                s.name = pn.clone();
                s.title = pn;
            }
        }
        s
    } else {
        // Configure git user from global git config or defaults
        let git_name =
            git_ops::git_config_get("user.name").unwrap_or_else(|_| "AIDA User".to_string());
        let git_email =
            git_ops::git_config_get("user.email").unwrap_or_else(|_| "aida@localhost".to_string());
        git_ops::configure_user(&store_dir, &git_name, &git_email)?;

        // Add remote if specified
        if let Some(remote) = registry_remote {
            // Check if remote already exists
            let has_remote = std::process::Command::new("git")
                .current_dir(&store_dir)
                .args(["remote", "get-url", "origin"])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

            if !has_remote {
                std::process::Command::new("git")
                    .current_dir(&store_dir)
                    .args(["remote", "add", "origin", remote])
                    .output()?;
                println!("  {} remote: {}", "Added".green(), remote.white().bold());
            }
        }

        // Initialize the git backend (creates objects/ and metadata.yaml)
        let backend = aida_core::GitBackend::new(&store_dir)?;
        let mut store = aida_core::models::RequirementsStore::new();
        // Project name from --name or cwd basename. trace:BUG-25 | ai:claude
        let project_name = name
            .map(|s| s.to_string())
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .and_then(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
            })
            .unwrap_or_default();
        if !project_name.is_empty() {
            store.name = project_name.clone();
            store.title = project_name;
        }
        seed_meta_requirements(&mut store)?;
        backend.save(&store)?;
        println!(
            "  {} {}",
            "Created".green(),
            format!("{store_rel}/metadata.yaml").white().bold()
        );
        println!(
            "  {} {}",
            "Created".green(),
            format!("{store_rel}/objects/").white().bold()
        );

        // Create initial commit
        git_ops::add(&store_dir, &["metadata.yaml"])?;
        std::fs::create_dir_all(store_dir.join("objects"))?;
        // Create a .gitkeep so objects/ is tracked
        std::fs::write(store_dir.join("objects/.gitkeep"), "")?;
        git_ops::add(&store_dir, &["objects/.gitkeep"])?;

        // Create .gitignore for node-local files
        let gitignore_content = "# Node-local state (not shared)\n.aida/\n*.lock\n";
        std::fs::write(store_dir.join(".gitignore"), gitignore_content)?;
        git_ops::add(&store_dir, &[".gitignore"])?;

        git_ops::commit(&store_dir, "chore: initialize AIDA distributed store")?;

        // If we have a remote, push the initial commit and register the node
        if registry_remote.is_some() {
            let branch = git_ops::current_branch(&store_dir).unwrap_or_else(|_| "main".to_string());

            // Push initial commit
            match git_ops::push(&store_dir, "origin", &branch) {
                Ok(true) => {
                    println!("  {} initial commit to remote", "Pushed".green(),);
                }
                Ok(false) => {
                    // Remote has content — pull first then push
                    git_ops::pull_rebase(&store_dir, "origin", &branch)?;
                    git_ops::push(&store_dir, "origin", &branch)?;
                    println!("  {} with remote and pushed", "Synced".green(),);
                }
                Err(e) => {
                    eprintln!("  {} Failed to push to remote: {}", "Warning:".yellow(), e);
                    eprintln!(
                        "  You can push later with: cd {} && git push -u origin {}",
                        store_rel, branch
                    );
                }
            }

            // Register this node
            let hostname = hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "unknown".to_string());

            match git_ops::register_node(&store_dir, 1, &hostname) {
                Ok(node_id) => {
                    println!(
                        "  {} node {} ({})",
                        "Registered".green(),
                        node_id.to_string().white().bold(),
                        hostname
                    );
                }
                Err(e) => {
                    eprintln!("  {} Node registration failed: {}", "Warning:".yellow(), e);
                    eprintln!("  You can register later when the remote is available.");
                }
            }
        } else {
            println!();
            println!("  {} No --registry-remote specified.", "Note:".yellow());
            println!("  The store is local-only until you add a remote:");
            println!("    cd {store_rel} && git remote add origin <url>");
            println!("    aida init --distributed --registry-remote <url>");
        }
        store
    };

    // Create .aida/ config in the project root (not the store)
    std::fs::create_dir_all(&aida_dir)?;
    let config_content = format!(
        "# AIDA distributed mode configuration\n\
         [deployment]\n\
         mode = \"distributed\"\n\
         # STORY-676: a separate store repo at this path (relative to the project\n\
         # root, or absolute); code repos pointing at the same path share a store.\n\
         store_path = \"{store_rel}\"\n\
         store_type = \"sibling\"\n\
         \n"
    ) + "[store.sync]\n\
         # Auto-push store commits after local writes. Values: manual,\n\
         # session-end, per-write, periodic. `periodic` is reserved until\n\
         # aida-worker (EPIC-30) ships.\n\
         auto_push = \"manual\"\n\
         \n\
         # trace:EPIC-1-052 Phase 2 | ai:claude\n\
         # How `aida add` chooses between agreed-id blocks and node-aware ids:\n\
         #   node-aware-only      — never use blocks; always FR-<NODE>-<SEQ>\n\
         #   blocks-then-fallback — try block first; fall through silently (default)\n\
         #   blocks-only          — error if no block is allocated for the type\n\
         [id_format]\n\
         policy = \"blocks-then-fallback\"\n\
         \n\
         # trace:TASK-281 | ai:claude\n\
         # Auto-claim a fresh block when aggregate remaining IDs drop below\n\
         # threshold. On by default (threshold 20, size 100). Opt out with:\n\
         #   [block_allocation]\n\
         #   auto_claim = false\n\
         # Per-type override (e.g. larger BUG blocks):\n\
         #   [block_allocation.bug]\n\
         #   auto_claim_threshold = 50\n\
         #   auto_claim_size = 200\n";
    // TASK-304: scaffold the [ultraplan] cadence block (mode = on-demand).
    let config_content = config_content.to_string() + init_ultraplan_config_section();
    // Commented [intake] example block (defaults are safe; discoverability only).
    // trace:TASK-760 | ai:claude
    let config_content = config_content + init_intake_config_section();
    // STORY-714/TASK-985: warm-pool ON by default (escape hatches documented).
    let config_content = config_content + init_worktree_pool_config_section();
    // STORY-760: commented [store.sync] mirror_remotes fan-out stub.
    let config_content = config_content + init_store_mirror_config_section();
    std::fs::write(aida_dir.join("config.toml"), &config_content)?;

    // Create docs/plans/ for plan archive (per CLAUDE.md convention).
    std::fs::create_dir_all(cwd.join("docs/plans"))?;
    ensure_plan_template_scaffold(&cwd.join("docs/plans"), force)?;

    // Run the shared workflow scaffolding (skills, hooks, mcp, codex).
    let storage_label = format!(
        "{}{}Git-canonical store (separate store repo at {})",
        format!("{store_rel}/").white().bold(),
        " ".repeat(20),
        store_rel
    );
    complete_init_scaffolding(
        &cwd,
        &store,
        agent,
        no_skills,
        no_hooks,
        force,
        std::path::PathBuf::from("aida-store"),
        &storage_label,
        verbose,
        // Genuinely-new sibling first-init: commit scaffolding as before (no
        // bootstrap-clone path here). trace:BUG-570
        false,
    )?;

    println!();
    if attaching {
        // trace:STORY-674 | ai:claude
        println!(
            "  {} Attached to existing store at {} ({} requirement(s)). Run {} to see them.",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            store_dir.display(),
            existing_count,
            "aida list".cyan()
        );
    } else {
        println!("  {}:", "Push code + store together".bold());
        println!(
            "    {}                        push your branch and the orphan store in one go",
            "aida push".cyan()
        );
    }
    println!();

    Ok(())
}
