// trace:TASK-3428 | ai:codex
use crate::{
    find_project_root, resolve_aida_exe, Cli, Colorize, Context, IsTerminal, Result,
    ASCIINEMA_SLUG_MAX_CHARS, ASCIINEMA_WRAPPED_ENV,
};

pub(crate) fn maybe_run_asciinema_wrapper(raw_args: &[String], cli: &Cli) -> Result<Option<i32>> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!(
            "{} --asciinema requested but stdin/stdout is not a TTY; running without recording.",
            crate::glyph(crate::glyphs::Glyph::Info).cyan()
        );
        return Ok(None);
    }

    if !asciinema_available() {
        eprintln!(
            "{} --asciinema requested but `asciinema` is not installed or not on PATH; running without recording.",
            crate::glyph(crate::glyphs::Glyph::Info).cyan()
        );
        return Ok(None);
    }

    let aida_exe = resolve_aida_exe();
    let inner_args = strip_asciinema_wrapper_args(raw_args);
    let cast_path = resolve_cast_output_path(cli.globals.cast_out.as_ref(), &inner_args)?;
    if let Some(parent) = cast_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating cast output directory {}", parent.display()))?;
    }
    let title = cli
        .globals
        .cast_title
        .clone()
        .unwrap_or_else(|| default_cast_title(&inner_args));
    let command = shell_command_for_asciinema(&aida_exe, &inner_args);

    let status = std::process::Command::new("asciinema")
        .args(["rec", "--command", &command, "--title", &title])
        .arg(&cast_path)
        .env(ASCIINEMA_WRAPPED_ENV, "1")
        .status()
        .context("could not invoke `asciinema rec`")?;

    if status.success() {
        println!("Cast saved: {}", cast_path.display());
    }
    Ok(Some(status.code().unwrap_or(1)))
}

pub(crate) fn asciinema_available() -> bool {
    std::process::Command::new("asciinema")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub(crate) fn strip_asciinema_wrapper_args(raw_args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(bin) = raw_args.first() {
        out.push(bin.clone());
    }
    let mut i = 1;
    while i < raw_args.len() {
        let arg = &raw_args[i];
        match arg.as_str() {
            "--asciinema" => {
                i += 1;
            }
            "--cast-out" | "--cast-title" => {
                i += 2;
            }
            _ if arg.starts_with("--cast-out=") || arg.starts_with("--cast-title=") => {
                i += 1;
            }
            _ => {
                out.push(arg.clone());
                i += 1;
            }
        }
    }
    out
}

pub(crate) fn resolve_cast_output_path(
    override_path: Option<&std::path::PathBuf>,
    inner_args: &[String],
) -> Result<std::path::PathBuf> {
    if let Some(path) = override_path {
        return Ok(path.clone());
    }
    let base_dir =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let stamp = chrono::Utc::now().format("%Y-%m-%dT%H%M%SZ");
    let slug = asciinema_command_slug(inner_args);
    Ok(base_dir
        .join(".aida")
        .join("casts")
        .join(format!("{stamp}-{slug}.cast")))
}

pub(crate) fn derive_semantic_title(inner_args: &[String]) -> Option<String> {
    let has_queue_work =
        inner_args.iter().any(|arg| arg == "queue") && inner_args.iter().any(|arg| arg == "work");
    let has_pr_ship =
        inner_args.iter().any(|arg| arg == "pr") && inner_args.iter().any(|arg| arg == "ship");

    // trace:TASK-3114 | ai:codex
    if inner_args
        .windows(2)
        .any(|pair| pair[0] == "pr" && pair[1] == "review")
    {
        let target = inner_args
            .iter()
            .skip_while(|arg| *arg != "review")
            .nth(1)?;
        let upper = target.to_ascii_uppercase();
        if upper.bytes().all(|byte| byte.is_ascii_digit()) && !upper.is_empty() {
            return Some(format!("AIDA pr review: PR-{upper}"));
        }
        if ["PR-", "MR-"].iter().any(|prefix| {
            upper.strip_prefix(*prefix).is_some_and(|number| {
                !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
            })
        }) {
            return Some(format!("AIDA pr review: {upper}"));
        }
        return None;
    }

    if has_queue_work {
        let mut specs = Vec::new();
        let mut batch_name = None;

        let mut i = 1;
        let spec_re = regex::Regex::new(r"(?i)^[a-z]+-\d+$").unwrap();
        while i < inner_args.len() {
            let arg = &inner_args[i];
            if arg == "--batch" && i + 1 < inner_args.len() {
                batch_name = Some(inner_args[i + 1].clone());
                i += 2;
                continue;
            } else if let Some(val) = arg.strip_prefix("--batch=") {
                batch_name = Some(val.to_string());
                i += 1;
                continue;
            }

            if spec_re.is_match(arg) {
                specs.push(arg.to_uppercase());
            }
            i += 1;
        }

        if !specs.is_empty() {
            return Some(format!("AIDA drain: {}", specs.join(", ")));
        } else if let Some(batch) = batch_name {
            return Some(format!("AIDA drain: batch {batch}"));
        }
    } else if has_pr_ship {
        let pr_re = regex::Regex::new(r"^\d+$").unwrap();
        for arg in inner_args.iter().skip(1) {
            if pr_re.is_match(arg) {
                return Some(format!("AIDA pr ship: PR-{arg}"));
            }
        }
    }
    None
}

pub(crate) fn default_cast_title(inner_args: &[String]) -> String {
    if let Some(semantic) = derive_semantic_title(inner_args) {
        return semantic;
    }
    let rest = inner_args
        .iter()
        .skip(1)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    if rest.trim().is_empty() {
        "aida".to_string()
    } else {
        format!("aida {rest}")
    }
}

pub(crate) fn slugify_str(s: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

pub(crate) fn derive_spec_aware_slug(inner_args: &[String]) -> Option<String> {
    let mut parts = Vec::new();
    let mut i = 1;

    let has_pr_ship =
        inner_args.iter().any(|arg| arg == "pr") && inner_args.iter().any(|arg| arg == "ship");
    // trace:TASK-3114 | ai:codex
    let has_pr_review = inner_args
        .windows(2)
        .any(|pair| pair[0] == "pr" && pair[1] == "review");
    let spec_re = regex::Regex::new(r"(?i)^[a-z]+-\d+$").unwrap();
    let pr_re = regex::Regex::new(r"^\d+$").unwrap();

    while i < inner_args.len() {
        let arg = &inner_args[i];

        if arg == "--batch" {
            if i + 1 < inner_args.len() {
                let val = slugify_str(&inner_args[i + 1]);
                parts.push(format!("batch-{val}"));
                i += 2;
                continue;
            }
        } else if let Some(raw) = arg.strip_prefix("--batch=") {
            let val = slugify_str(raw);
            parts.push(format!("batch-{val}"));
            i += 1;
            continue;
        }

        if spec_re.is_match(arg) {
            parts.push(arg.to_lowercase());
        } else if (has_pr_ship || has_pr_review) && pr_re.is_match(arg) {
            parts.push(format!("pr-{arg}"));
        }

        i += 1;
    }

    if !parts.is_empty() {
        let mut seen = std::collections::HashSet::new();
        let mut unique_parts = Vec::new();
        for p in parts {
            if seen.insert(p.clone()) {
                unique_parts.push(p);
            }
        }
        Some(unique_parts.join("-"))
    } else {
        None
    }
}

pub(crate) fn asciinema_command_slug(inner_args: &[String]) -> String {
    if let Some(spec_slug) = derive_spec_aware_slug(inner_args) {
        return truncate_asciinema_slug(&spec_slug);
    }

    // Generic fallback: use subcommand + first non-flag arg
    let pos_args: Vec<String> = inner_args
        .iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .map(|arg| slugify_str(arg))
        .filter(|arg| !arg.is_empty())
        .collect();

    let fallback_slug = if pos_args.len() >= 2 {
        format!("{}-{}", pos_args[0], pos_args[1])
    } else if pos_args.len() == 1 {
        pos_args[0].clone()
    } else {
        "aida".to_string()
    };

    truncate_asciinema_slug(&fallback_slug)
}

pub(crate) fn truncate_asciinema_slug(slug: &str) -> String {
    let count = slug.chars().count();
    if count <= ASCIINEMA_SLUG_MAX_CHARS {
        return slug.to_string();
    }
    let keep = ASCIINEMA_SLUG_MAX_CHARS.saturating_sub("-trunc".len());
    let mut truncated = slug.chars().take(keep).collect::<String>();
    truncated = truncated.trim_end_matches('-').to_string();
    format!("{truncated}-trunc")
}

pub(crate) fn shell_command_for_asciinema(
    aida_exe: &std::path::Path,
    inner_args: &[String],
) -> String {
    std::iter::once(aida_exe.to_string_lossy().to_string())
        .chain(inner_args.iter().skip(1).cloned())
        .map(|arg| shell_quote(&arg))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn shell_quote(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_string();
    }
    if arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '='))
    {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', "'\"'\"'"))
}
