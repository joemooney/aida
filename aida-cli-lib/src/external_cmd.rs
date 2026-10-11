//! Git-style external subcommands: an unrecognised `aida <name>` runs an
//! executable `aida-<name>` found on `PATH`.
//!
//! This is the extension seat of the core/porcelain boundary: convenience
//! composition can ship as a separate `aida-<name>` program without changing
//! core. Resolution order is fixed — built-in subcommand, then built-in alias
//! (clap aliases and the pre-clap argv rewrites), then user alias (interactive
//! human callers only), then external. An external can never shadow anything
//! earlier in that list: the dispatcher only consults `PATH` once argv has been
//! through every earlier stage and its head is still not a real subcommand.
//!
//! The extension receives its context through the environment so it never has
//! to rediscover it: `AIDA_PROJECT_ROOT`, `AIDA_STORE_PATH` (both removed when
//! the caller is outside an AIDA project), `AIDA_OUTPUT_FORMAT` (resolved),
//! `AIDA_CALLER_KIND` (`human` / `agent` / `headless`) and `AIDA_BIN` (the
//! invoking binary, so the extension calls the same version back).
//!
//! On Unix the extension replaces the `aida` process (`exec`), so signals, the
//! TTY and the exit status pass straight through. On Windows it is spawned and
//! its exit code propagated; `aida-<name>.exe`, `.cmd` and `.bat` resolve.
// trace:STORY-1641 | ai:claude

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// File-name prefix every external command carries.
pub(crate) const PREFIX: &str = "aida-";

/// Companion binaries that ship with AIDA itself. They sit next to `aida` on
/// `PATH` but are not extensions (no `--help` contract, long-running servers),
/// so they are never discovered or dispatched as `aida <name>`.
// trace:STORY-1641 | ai:claude
const RESERVED_COMPANIONS: &[&str] = &["server", "generate-types"];

/// Environment variable names handed to an extension.
pub(crate) const ENV_PROJECT_ROOT: &str = "AIDA_PROJECT_ROOT";
pub(crate) const ENV_STORE_PATH: &str = "AIDA_STORE_PATH";
pub(crate) const ENV_OUTPUT_FORMAT: &str = "AIDA_OUTPUT_FORMAT";
pub(crate) const ENV_CALLER_KIND: &str = "AIDA_CALLER_KIND";
pub(crate) const ENV_BIN: &str = "AIDA_BIN";

/// One installed external command.
// trace:STORY-1641 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct External {
    /// The name typed after `aida` (`hello` for `aida-hello`).
    pub name: String,
    /// The resolved executable (first match on `PATH`).
    pub path: PathBuf,
}

/// The context an extension sees, before it is written into its environment.
// trace:STORY-1641 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtensionContext {
    pub project_root: Option<PathBuf>,
    pub store_path: Option<PathBuf>,
    pub output_format: &'static str,
    pub caller_kind: &'static str,
    pub aida_bin: PathBuf,
}

/// A name may become `aida-<name>` only when it is a plain command word: no
/// path separators, no leading `-` (a flag) or `.` (a hidden/relative path).
// trace:STORY-1641 | ai:claude
pub(crate) fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Every name an external may never take: real subcommands and their clap
/// aliases, clap's own `help`, and AIDA's companion binaries.
// trace:STORY-1641 | ai:claude
pub(crate) fn shadowed_names() -> HashSet<String> {
    let mut names = crate::user_alias::real_subcommand_names();
    names.insert("help".to_string());
    for c in RESERVED_COMPANIONS {
        names.insert((*c).to_string());
    }
    names
}

/// The external name argv would dispatch to, if any: `argv[1]` after every
/// earlier resolution stage, when it is a valid command word that is not
/// shadowed. Pure — does not look at `PATH`.
// trace:STORY-1641 | ai:claude
pub(crate) fn dispatch_name<'a>(argv: &'a [String], shadowed: &HashSet<String>) -> Option<&'a str> {
    let head = argv.get(1)?.as_str();
    (is_valid_name(head) && !shadowed.contains(head)).then_some(head)
}

/// The file names `aida-<name>` may take on this platform, in lookup order.
fn candidate_file_names(name: &str) -> Vec<String> {
    if cfg!(windows) {
        ["exe", "cmd", "bat"]
            .iter()
            .map(|ext| format!("{PREFIX}{name}.{ext}"))
            .collect()
    } else {
        vec![format!("{PREFIX}{name}")]
    }
}

/// The command name a `PATH` entry file name provides, if it is an external.
fn name_from_file_name(file_name: &str) -> Option<String> {
    let rest = file_name.strip_prefix(PREFIX)?;
    let name = if cfg!(windows) {
        let lower = rest.to_ascii_lowercase();
        let ext_len = [".exe", ".cmd", ".bat"]
            .iter()
            .find(|ext| lower.ends_with(*ext))
            .map(|ext| ext.len())?;
        &rest[..rest.len() - ext_len]
    } else {
        rest
    };
    is_valid_name(name).then(|| name.to_string())
}

/// True for a regular file this user may execute (the crate's shared check).
fn is_executable(path: &Path) -> bool {
    crate::is_executable(path)
}

/// Resolve `aida-<name>` against an explicit `PATH` value. First match wins.
// trace:STORY-1641 | ai:claude
pub(crate) fn find_in(name: &str, path_var: Option<&OsStr>) -> Option<PathBuf> {
    if !is_valid_name(name) {
        return None;
    }
    let path_var = path_var?;
    for dir in std::env::split_paths(path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for file in candidate_file_names(name) {
            let candidate = dir.join(&file);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Every installed external on an explicit `PATH`, excluding shadowed names.
/// The first directory providing a name wins (as it would at dispatch).
/// Sorted by name.
// trace:STORY-1641 | ai:claude
pub(crate) fn discover_in(path_var: Option<&OsStr>, shadowed: &HashSet<String>) -> Vec<External> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<External> = Vec::new();
    let Some(path_var) = path_var else {
        return out;
    };
    for dir in std::env::split_paths(path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<(String, PathBuf)> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let file_name = e.file_name();
                let name = name_from_file_name(file_name.to_str()?)?;
                Some((name, e.path()))
            })
            .collect();
        names.sort();
        for (name, path) in names {
            if shadowed.contains(&name) || seen.contains(&name) || !is_executable(&path) {
                continue;
            }
            seen.insert(name.clone());
            out.push(External { name, path });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Every installed external on the process `PATH`.
// trace:STORY-1641 | ai:claude
pub(crate) fn discover() -> Vec<External> {
    discover_in(std::env::var_os("PATH").as_deref(), &shadowed_names())
}

/// The caller classification handed to an extension. Mirrors the user-alias
/// gate: an agent marker, a headless drain, or a non-terminal caller is never
/// `human`; `headless` is reported ahead of `agent` when both apply.
// trace:STORY-1641 | ai:claude
pub(crate) fn caller_kind_from(
    agent_type: Option<&str>,
    headless: Option<&str>,
    interactive_tty: bool,
) -> &'static str {
    if headless == Some("1") {
        return "headless";
    }
    if agent_type.is_some_and(|v| !v.is_empty()) {
        return "agent";
    }
    if interactive_tty {
        "human"
    } else {
        "agent"
    }
}

/// The resolved output format label: an explicit `AIDA_OUTPUT_FORMAT` wins;
/// otherwise the same agent-mode default every built-in uses (`toon` when
/// piped or `AIDA_AGENT_OUTPUT` is truthy, `human` at a terminal).
// trace:STORY-1641 | ai:claude
pub(crate) fn output_format_label(
    format_env: Option<&str>,
    agent_output_env: Option<&str>,
    stdout_is_tty: bool,
) -> &'static str {
    use crate::cli::OutputFormat;
    match crate::output_format::parse_output_format(format_env) {
        Some(OutputFormat::Human) => "human",
        Some(OutputFormat::Toon) => "toon",
        Some(OutputFormat::Json) => "json",
        None => {
            if crate::output_format::agent_output_mode_from(agent_output_env, stdout_is_tty) {
                "toon"
            } else {
                "human"
            }
        }
    }
}

/// The AIDA project root and store for `start`: the nearest enclosing git
/// checkout that carries `.aida/config.toml`. The store is the config's
/// `store_path` (or `.aida-store`) when it exists there, else the shared
/// resolver's answer (a linked worktree's store lives in the main checkout).
/// Both are `None` outside an AIDA project.
// trace:STORY-1641 | ai:claude
pub(crate) fn project_and_store_from(start: &Path) -> (Option<PathBuf>, Option<PathBuf>) {
    let Ok(root) = crate::kernel::find_project_root_from(start) else {
        return (None, None);
    };
    let config = root.join(".aida").join("config.toml");
    let Ok(body) = std::fs::read_to_string(&config) else {
        return (None, None);
    };
    let mut candidates = aida_core::store_locate::store_path_candidates(&body);
    candidates.push(".aida-store".to_string());
    let local = candidates
        .iter()
        .filter(|v| !v.is_empty())
        .map(|v| root.join(v))
        .find(|p| p.is_dir());
    let store = local.or_else(|| aida_core::store_locate::detect_distributed_store_from(&root));
    (Some(root), store)
}

/// Gather the extension context from the live process.
// trace:STORY-1641 | ai:claude
pub(crate) fn current_context() -> ExtensionContext {
    use std::io::IsTerminal;
    let (project_root, store_path) = match std::env::current_dir() {
        Ok(cwd) => project_and_store_from(&cwd),
        Err(_) => (None, None),
    };
    let agent_type = std::env::var("AIDA_AGENT_TYPE").ok();
    let headless = std::env::var("AIDA_HEADLESS").ok();
    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let format_env = std::env::var(ENV_OUTPUT_FORMAT).ok();
    let agent_output_env = std::env::var("AIDA_AGENT_OUTPUT").ok();
    ExtensionContext {
        project_root,
        store_path,
        output_format: output_format_label(
            format_env.as_deref(),
            agent_output_env.as_deref(),
            std::io::stdout().is_terminal(),
        ),
        caller_kind: caller_kind_from(agent_type.as_deref(), headless.as_deref(), tty),
        aida_bin: crate::aida_bin::running_executable_fallback(),
    }
}

/// Write `ctx` into the environment of `cmd`.
fn apply_context(cmd: &mut std::process::Command, ctx: &ExtensionContext) {
    match &ctx.project_root {
        Some(p) => cmd.env(ENV_PROJECT_ROOT, p),
        None => cmd.env_remove(ENV_PROJECT_ROOT),
    };
    match &ctx.store_path {
        Some(p) => cmd.env(ENV_STORE_PATH, p),
        None => cmd.env_remove(ENV_STORE_PATH),
    };
    cmd.env(ENV_OUTPUT_FORMAT, ctx.output_format);
    cmd.env(ENV_CALLER_KIND, ctx.caller_kind);
    cmd.env(ENV_BIN, &ctx.aida_bin);
}

/// Installed externals within a small edit distance of `name`, nearest first.
// trace:STORY-1641 | ai:claude
pub(crate) fn near_matches(name: &str, installed: &[External]) -> Vec<External> {
    let mut hits: Vec<(usize, External)> = installed
        .iter()
        .filter(|e| e.name != name)
        .map(|e| (crate::help_catalog::levenshtein(name, &e.name), e.clone()))
        .filter(|(d, _)| *d <= 2)
        .collect();
    hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    hits.into_iter().map(|(_, e)| e).collect()
}

/// Dispatch `argv` (after every earlier resolution stage) to an external when
/// its head names one. `Ok(None)` means "not an external — let clap handle
/// it" (after printing a near-match hint when one exists). On Unix a
/// successful dispatch never returns; on Windows it returns the child's code.
// trace:STORY-1641 | ai:claude
pub(crate) fn maybe_dispatch(argv: &[String]) -> Result<Option<i32>> {
    let shadowed = shadowed_names();
    let Some(name) = dispatch_name(argv, &shadowed) else {
        return Ok(None);
    };
    let path_var = std::env::var_os("PATH");
    let Some(exe) = find_in(name, path_var.as_deref()) else {
        let installed = discover_in(path_var.as_deref(), &shadowed);
        if let Some(best) = near_matches(name, &installed).first() {
            eprintln!(
                "hint: an installed extension has a similar name: `aida {}` ({})",
                best.name,
                best.path.display()
            );
        }
        return Ok(None);
    };
    run_external(&exe, &argv[2..]).map(Some)
}

/// Run an external with `args`, its context in the environment.
// trace:STORY-1641 | ai:claude
pub(crate) fn run_external(exe: &Path, args: &[String]) -> Result<i32> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args);
    apply_context(&mut cmd, &current_context());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        anyhow::bail!("could not run `{}`: {err}", exe.display());
    }
    #[cfg(not(unix))]
    {
        let status = cmd
            .status()
            .map_err(|err| anyhow::anyhow!("could not run `{}`: {err}", exe.display()))?;
        Ok(status.code().unwrap_or(1))
    }
}

/// `aida help <name>` for an external: delegate to `aida-<name> --help`.
/// `None` when no such external is installed (or the name is shadowed).
// trace:STORY-1641 | ai:claude
pub(crate) fn help_for(name: &str) -> Option<Result<i32>> {
    if !is_valid_name(name) || shadowed_names().contains(name) {
        return None;
    }
    let exe = find_in(name, std::env::var_os("PATH").as_deref())?;
    Some(run_external(&exe, &["--help".to_string()]))
}

/// The "External commands" section of `aida help` / `aida help --all`.
/// Prints nothing when no extension is installed.
// trace:STORY-1641 | ai:claude
pub(crate) fn print_help_section() {
    use colored::Colorize;
    let installed = discover();
    if installed.is_empty() {
        return;
    }
    println!("{}", "External commands".cyan().bold());
    let width = installed
        .iter()
        .map(|e| e.name.len())
        .max()
        .unwrap_or(0)
        .max(10);
    for e in &installed {
        println!(
            "  {:<width$} {} {}",
            e.name.green(),
            "external".dimmed(),
            e.path.display(),
            width = width,
        );
    }
    println!();
}

/// True when `a` and `b` name the same file (after resolving symlinks).
// trace:STORY-1641 | ai:claude
pub(crate) fn same_binary(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

#[cfg(test)]
#[path = "tests/story_1641_external_cmd_tests.rs"]
mod story_1641_external_cmd_tests;
