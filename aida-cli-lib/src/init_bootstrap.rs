//! `aida init <DIR>` — bootstrap a brand-new project from nothing.
//!
//! AIDA owns the SEQUENCE and its ordering: create the directory, scaffold the
//! language project (delegated to the NATIVE tool — AIDA ships no templates),
//! ensure a git repo with a first commit, run the standard `aida init` inside
//! it, then create/attach the remote and push the code branch and `aida-store`
//! in that order. The ordering is the value: creating the remote first (with a
//! README) leaves divergent history on day one, and pushing the store leg last
//! or never is a silent failure that surfaces as a missing store on the second
//! clone. Each step no-ops when already done, so a failed run is re-runnable.
// trace:STORY-780 | ai:claude

use anyhow::{bail, Context, Result};
use colored::Colorize;
use std::path::{Path, PathBuf};

/// The operator-decided surface: positional DIR, native-tool `--lang`,
/// private-by-default `--github`, bring-your-own `--remote <url>`.
pub(crate) struct BootstrapPlan {
    pub dir: PathBuf,
    pub lang: Option<Lang>,
    pub github: bool,
    pub public: bool,
    pub remote: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Lang {
    Rust,
    Python,
    Node,
}

impl Lang {
    pub(crate) fn parse(s: &str) -> Result<Lang> {
        match s.to_ascii_lowercase().as_str() {
            "rust" => Ok(Lang::Rust),
            "python" => Ok(Lang::Python),
            "node" | "javascript" | "js" => Ok(Lang::Node),
            other => bail!(
                "unknown --lang `{other}` — supported: rust (cargo init), python (uv init), node (npm init -y)"
            ),
        }
    }

    /// The native tool this language delegates to. AIDA never ships its own
    /// language templates — the moment it does, it owns keeping them current.
    pub(crate) fn tool(self) -> &'static str {
        match self {
            Lang::Rust => "cargo",
            Lang::Python => "uv",
            Lang::Node => "npm",
        }
    }

    pub(crate) fn scaffold_args(self) -> &'static [&'static str] {
        match self {
            Lang::Rust => &["init"],
            Lang::Python => &["init"],
            Lang::Node => &["init", "-y"],
        }
    }

    fn install_hint(self) -> &'static str {
        match self {
            Lang::Rust => "https://rustup.rs",
            Lang::Python => "https://docs.astral.sh/uv/getting-started/installation/",
            Lang::Node => "https://nodejs.org",
        }
    }
}

fn tool_on_path(tool: &str) -> bool {
    which_ok(tool)
}

#[cfg(unix)]
fn which_ok(tool: &str) -> bool {
    std::process::Command::new("which")
        .arg(tool)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn which_ok(tool: &str) -> bool {
    std::process::Command::new("where")
        .arg(tool)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Everything that can refuse must refuse BEFORE the first mkdir, so a failed
/// pre-flight leaves the filesystem untouched.
pub(crate) fn preflight(plan: &BootstrapPlan) -> Result<()> {
    if !tool_on_path("git") {
        bail!("git is required to bootstrap a project and was not found on PATH");
    }
    if let Some(lang) = plan.lang {
        if !tool_on_path(lang.tool()) {
            bail!(
                "--lang needs `{}` on PATH and it was not found — install it first: {}",
                lang.tool(),
                lang.install_hint()
            );
        }
    }
    if plan.github && !tool_on_path("gh") {
        bail!("--github needs the `gh` CLI on PATH — install it first: https://cli.github.com");
    }
    if plan.dir.exists() {
        let non_empty = std::fs::read_dir(&plan.dir)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);
        let is_git = plan.dir.join(".git").exists();
        if non_empty && !is_git {
            bail!(
                "{} exists, is not empty, and is not a git repository — refusing to scaffold over it. \
                 Pick a new directory, or `git init` there yourself if it really is your project.",
                plan.dir.display()
            );
        }
    }
    Ok(())
}

fn run_in(dir: &Path, program: &str, args: &[&str]) -> Result<()> {
    let status = std::process::Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .with_context(|| format!("could not launch `{program}`"))?;
    if !status.success() {
        bail!("`{program} {}` failed in {}", args.join(" "), dir.display());
    }
    Ok(())
}

fn git_head_exists(dir: &Path) -> bool {
    std::process::Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(dir)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Steps 1–4: mkdir → language scaffold → ensure git repo → first commit.
/// Each step no-ops when its outcome already holds, so re-running after a
/// mid-sequence failure resumes instead of erroring.
pub(crate) fn create_and_scaffold(plan: &BootstrapPlan) -> Result<()> {
    let dir = &plan.dir;
    if !dir.exists() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
        println!("  {} created {}", "+".green(), dir.display());
    }
    let dir_empty = std::fs::read_dir(dir)
        .map(|mut d| d.next().is_none())
        .unwrap_or(true);
    if let Some(lang) = plan.lang {
        // Only scaffold into an empty dir — a resumed run (or a pre-existing
        // repo) keeps whatever the first pass produced.
        if dir_empty {
            run_in(dir, lang.tool(), lang.scaffold_args())?;
            println!(
                "  {} scaffolded with `{} {}`",
                "+".green(),
                lang.tool(),
                lang.scaffold_args().join(" ")
            );
        } else {
            println!(
                "  {} directory not empty — skipping `{} {}`",
                "Note:".dimmed(),
                lang.tool(),
                lang.scaffold_args().join(" ")
            );
        }
    }
    if !dir.join(".git").exists() {
        run_in(dir, "git", &["init"])?;
        println!("  {} git repository initialized", "+".green());
    }
    if !git_head_exists(dir) {
        // A first commit must exist before `aida init` runs (the store
        // worktree machinery needs a born HEAD). Scaffolded files are the
        // commit when present; otherwise an empty commit keeps AIDA from
        // owning any content it didn't write.
        run_in(dir, "git", &["add", "-A"])?;
        let has_staged = !std::process::Command::new("git")
            .args(["diff", "--cached", "--quiet"])
            .current_dir(dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(true);
        if has_staged {
            run_in(
                dir,
                "git",
                &["commit", "-m", "chore: initial project scaffold"],
            )?;
        } else {
            run_in(
                dir,
                "git",
                &["commit", "--allow-empty", "-m", "chore: repository created"],
            )?;
        }
        println!("  {} first commit created", "+".green());
    }
    Ok(())
}

fn current_branch(dir: &Path) -> Result<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
        .context("could not read the current branch")?;
    if !out.status.success() {
        bail!("could not read the current branch in {}", dir.display());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn origin_exists(dir: &Path) -> bool {
    std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(dir)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Steps 6–8, run AFTER the standard init created the `aida-store` branch:
/// create/attach the remote, push the code branch, push the store branch —
/// in that order, every time. cwd is already the project dir here.
pub(crate) fn finish_remote(plan: &BootstrapPlan) -> Result<()> {
    let dir = std::env::current_dir().context("no current dir")?;
    let branch = current_branch(&dir)?;
    let name = plan
        .dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());

    if plan.github {
        if origin_exists(&dir) {
            println!(
                "  {} origin already configured — skipping repo creation",
                "Note:".dimmed()
            );
            run_in(&dir, "git", &["push", "-u", "origin", &branch])?;
        } else {
            let argv = crate::forge::ForgeKind::GitHub
                .repo_create_argv(&name, plan.public)
                .expect("github forge always yields a create argv");
            let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            run_in(&dir, argv_refs[0], &argv_refs[1..])?;
            println!(
                "  {} created {} GitHub repo `{}` and pushed `{}`",
                "+".green(),
                if plan.public { "public" } else { "private" },
                name,
                branch
            );
        }
    } else if let Some(url) = &plan.remote {
        if !origin_exists(&dir) {
            run_in(&dir, "git", &["remote", "add", "origin", url])?;
        }
        // Works for hosts with push-to-create (GitLab): the first push
        // creates the project.
        run_in(&dir, "git", &["push", "-u", "origin", &branch])?;
        println!("  {} pushed `{}` to {}", "+".green(), branch, url);
    } else {
        println!();
        println!(
            "  {} local-only project — when you add a remote, push BOTH legs:",
            "Note:".dimmed()
        );
        println!("      git push -u origin {branch}");
        println!("      git push -u origin aida-store");
        emit_enter_dir(&plan.dir);
        return Ok(());
    }

    // The store leg, always last and never skipped when a remote exists.
    run_in(&dir, "git", &["push", "-u", "origin", "aida-store"])?;
    println!(
        "  {} pushed `aida-store` (the requirements store)",
        "+".green()
    );
    println!();
    println!("  {} project ready:", "✓".green().bold());
    emit_enter_dir(&plan.dir);
    Ok(())
}

/// Hand the calling shell a `cd` into the new project — through the marked
/// eval channel when (and only when) the shell's wrapper advertises the
/// `init-cd` capability; otherwise a plain copy-paste hint. A wrapper that
/// speaks only the legacy verbs never sees markers from init, so nothing is
/// ever printed as comment noise.
fn emit_enter_dir(plan_dir: &Path) {
    let abs = std::env::current_dir().unwrap_or_else(|_| plan_dir.to_path_buf());
    let wrapper = std::env::var("AIDA_SHELL_WRAPPER").ok();
    if crate::shell_eval::marker_has_cap(wrapper.as_deref(), crate::shell_eval::INIT_CD_CAP) {
        let _eval = crate::shell_eval::EvalBlock::open_with(true);
        println!("cd '{}'", abs.display());
    } else {
        println!("      cd {}", plan_dir.display());
    }
}
