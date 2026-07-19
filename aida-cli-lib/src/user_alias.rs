//! User/project-defined `aida` command aliases (`aida alias add/list/remove`).
//!
//! An alias maps a short name to an `aida` command + args, so `aida <alias>
//! [extra args]` expands to the stored command with any trailing args appended.
//! Two scopes:
//!
//! - **personal** — `~/.aida/aliases.toml`, convenience parity with shell
//!   aliases but available in AIDA contexts where the shell rc is not loaded.
//! - **project** — `.aida/aliases.toml`, git-trackable + shareable across a
//!   team, available in CI / fresh clones / sandboxes.
//!
//! Precedence on a name clash: **project overrides personal overrides
//! built-in**. A user alias may NEVER shadow a real subcommand (`add` refuses).
//!
//! HARD INVARIANT (TASK-877): alias EXPANSION applies ONLY to interactive human
//! shells. Agent / headless / MCP / non-TTY callers MUST resolve canonical
//! commands and NEVER expand a user alias — the substrate's value is a stable
//! canonical surface legible to every vendor. `aida alias list` still SHOWS
//! user aliases to an agent (discoverability), but [`expand`] is gated on
//! [`is_interactive_human_caller`] so the dispatcher never expands them for a
//! non-human caller.
//!
//! Storage is TOML, written comment-preserving via `toml_edit` — mirrors the
//! `glyph_config` / `config_edit` patterns. The `[alias]` table maps
//! `name = "expansion"`, where the expansion is the command line *after* `aida`
//! (e.g. `approved = "list --status approved"`).
//!
//! trace:TASK-877 | ai:claude

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use toml_edit::{DocumentMut, Item, Table, Value};

/// Which `aliases.toml` an alias command targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Project-level `.aida/aliases.toml` (git-trackable, shareable).
    Project,
    /// User-level `~/.aida/aliases.toml` (personal).
    Personal,
}

impl Scope {
    /// Human label for `aida alias list` and messages.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Scope::Project => "project",
            Scope::Personal => "personal",
        }
    }
}

/// Resolve the requested scope, mirroring how other scoped commands choose:
/// an explicit `--project` / `--global` flag wins; otherwise default to project
/// when inside an AIDA project, else personal.
pub(crate) fn resolve_scope(project: bool, global: bool) -> Result<Scope> {
    if project && global {
        anyhow::bail!("pass at most one of --project / --global");
    }
    if global {
        return Ok(Scope::Personal);
    }
    if project {
        return Ok(Scope::Project);
    }
    // Default: project if inside an AIDA project, else personal.
    if crate::find_project_root().is_ok() {
        Ok(Scope::Project)
    } else {
        Ok(Scope::Personal)
    }
}

/// Resolve the `aliases.toml` path for `scope`. The project path is derived from
/// [`crate::find_project_root`]; the personal path from the home dir.
pub(crate) fn aliases_path_for(scope: Scope) -> Result<PathBuf> {
    match scope {
        Scope::Project => {
            let root = crate::find_project_root()
                .context("not inside an AIDA project — run `aida init`, or use --global")?;
            Ok(root.join(".aida").join("aliases.toml"))
        }
        Scope::Personal => {
            let home = home_dir()
                .ok_or_else(|| anyhow::anyhow!("could not resolve home directory for --global"))?;
            Ok(home.join(".aida").join("aliases.toml"))
        }
    }
}

/// The personal path if it resolves, else None (no home dir). Never errors —
/// used by the read-side [`load_table`] / [`expand`] which must degrade quietly.
fn personal_path_opt() -> Option<PathBuf> {
    Some(home_dir()?.join(".aida").join("aliases.toml"))
}

/// The project path if inside a project, else None. Never errors — read-side.
fn project_path_opt() -> Option<PathBuf> {
    crate::find_project_root()
        .ok()
        .map(|root| root.join(".aida").join("aliases.toml"))
}

/// Honors the test override so unit tests don't read the real home.
fn home_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(home) = std::env::var_os("AIDA_TEST_HOME") {
        return Some(PathBuf::from(home).join("home"));
    }
    dirs::home_dir()
}

/// Load an `aliases.toml` into an editable document, or a fresh empty one if the
/// file is absent. Parse errors surface (we don't clobber a malformed file).
fn load_doc(path: &Path) -> Result<DocumentMut> {
    match std::fs::read_to_string(path) {
        Ok(body) => body
            .parse::<DocumentMut>()
            .with_context(|| format!("failed to parse {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Write a document back, creating the parent dir if needed.
fn save_doc(path: &Path, doc: &DocumentMut) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    aida_core::write_atomic(path, doc.to_string())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Ensure `doc[key]` is a table, returning a mutable reference.
fn table_mut<'a>(doc: &'a mut DocumentMut, key: &str) -> &'a mut Table {
    if !doc.contains_table(key) {
        doc.insert(key, Item::Table(Table::new()));
    }
    doc[key]
        .as_table_mut()
        .expect("just-inserted/confirmed table")
}

/// Read the `[alias]` table at `path` into `(name, expansion)` pairs, sorted by
/// name. Missing file / missing table → empty. A malformed file is reported.
pub(crate) fn load_table(path: &Path) -> Result<Vec<(String, String)>> {
    let doc = load_doc(path)?;
    let mut out = Vec::new();
    if let Some(table) = doc.get("alias").and_then(Item::as_table) {
        for (name, item) in table.iter() {
            if let Some(s) = item.as_str() {
                out.push((name.to_string(), s.to_string()));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// The set of real top-level subcommand names + aliases, derived from the live
/// clap command tree (`Cli::command()`) — the single source of truth for "what
/// is a real subcommand". A user alias may not shadow any of these.
/// trace:TASK-877 | ai:claude
pub(crate) fn real_subcommand_names() -> std::collections::HashSet<String> {
    use clap::CommandFactory;
    let cmd = crate::cli::Cli::command();
    let mut names = std::collections::HashSet::new();
    for sub in cmd.get_subcommands() {
        names.insert(sub.get_name().to_string());
        for a in sub.get_all_aliases() {
            names.insert(a.to_string());
        }
    }
    names
}

/// A user-alias name must be a single non-flag token that does not collide with
/// a real subcommand. Returns Ok for a valid name or an error explaining the
/// refusal. trace:TASK-877 | ai:claude
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("alias name cannot be empty");
    }
    if name.starts_with('-') {
        anyhow::bail!("alias name cannot start with '-' (got `{name}`)");
    }
    if name.split_whitespace().count() != 1 {
        anyhow::bail!("alias name must be a single token (got `{name}`)");
    }
    if real_subcommand_names().contains(name) {
        anyhow::bail!(
            "`{name}` is a real `aida` subcommand — an alias may not shadow it; pick another name"
        );
    }
    Ok(())
}

/// `aida alias add <name> <command...>` — write `[alias] name = "<expansion>"`
/// to the scope's `aliases.toml`, preserving the rest of the file. The
/// expansion is the command line *after* `aida`. Refuses to shadow a real
/// subcommand. trace:TASK-877 | ai:claude
pub(crate) fn add(scope: Scope, name: &str, command_tokens: &[String]) -> Result<()> {
    validate_name(name)?;
    if command_tokens.is_empty() {
        anyhow::bail!(
            "alias `{name}` needs a command to expand to (e.g. `aida alias add {name} list --status approved`)"
        );
    }
    let expansion = command_tokens.join(" ");
    let path = aliases_path_for(scope)?;
    let mut doc = load_doc(&path)?;
    let table = table_mut(&mut doc, "alias");
    table.insert(name, Item::Value(Value::from(expansion.clone())));
    save_doc(&path, &doc)?;
    println!(
        "Added {} alias `{}` → `aida {}`",
        scope.label(),
        name,
        expansion
    );
    println!("  {}", path.display());
    Ok(())
}

/// `aida alias remove <name>` — drop `[alias] name` from the scope's file.
/// Errors if absent. Drops an emptied `[alias]` table.
/// trace:TASK-877 | ai:claude
pub(crate) fn remove(scope: Scope, name: &str) -> Result<()> {
    let path = aliases_path_for(scope)?;
    let mut doc = load_doc(&path)?;
    let mut removed = false;
    if let Some(table) = doc.get_mut("alias").and_then(Item::as_table_mut) {
        removed = table.remove(name).is_some();
        if table.is_empty() {
            doc.remove("alias");
        }
    }
    if removed {
        save_doc(&path, &doc)?;
        println!("Removed {} alias `{}`", scope.label(), name);
    } else {
        anyhow::bail!(
            "no {} alias named `{}` (in {})",
            scope.label(),
            name,
            path.display()
        );
    }
    Ok(())
}

/// A resolved user alias for the `aida alias list` registry.
/// trace:TASK-877 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct UserAliasRow {
    pub name: String,
    pub expansion: String,
    pub scope: Scope,
}

/// Read both scopes and return the effective set of user aliases, with project
/// overriding personal on a name clash. Sorted by name. trace:TASK-877
pub(crate) fn effective_user_aliases() -> Vec<UserAliasRow> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<String, UserAliasRow> = BTreeMap::new();
    // Personal first; project overwrites (project wins).
    if let Some(p) = personal_path_opt() {
        if let Ok(rows) = load_table(&p) {
            for (name, expansion) in rows {
                map.insert(
                    name.clone(),
                    UserAliasRow {
                        name,
                        expansion,
                        scope: Scope::Personal,
                    },
                );
            }
        }
    }
    if let Some(p) = project_path_opt() {
        if let Ok(rows) = load_table(&p) {
            for (name, expansion) in rows {
                map.insert(
                    name.clone(),
                    UserAliasRow {
                        name,
                        expansion,
                        scope: Scope::Project,
                    },
                );
            }
        }
    }
    map.into_values().collect()
}

/// True for an interactive human caller; false for an agent / headless / MCP /
/// non-TTY caller. This is the HARD-INVARIANT gate (TASK-877): only an
/// interactive human shell ever has its argv expanded against the user-alias
/// tables. An agent (`AIDA_AGENT_TYPE` set), a headless drain (`AIDA_HEADLESS`),
/// the MCP server path, or any non-TTY caller resolves the canonical surface.
/// trace:TASK-877 | ai:claude
pub(crate) fn is_interactive_human_caller() -> bool {
    if std::env::var_os("AIDA_AGENT_TYPE").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if std::env::var("AIDA_HEADLESS").as_deref() == Ok("1") {
        return false;
    }
    // Non-TTY (piped, CI, captured, or the MCP server's stdio transport) is
    // treated as non-human. The MCP `mcp-serve` path also never routes argv
    // through `expand` — it dispatches canonical tool calls — so user aliases
    // are doubly walled off from agents. trace:TASK-877 | ai:claude
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Maximum expansion hops before we declare a cycle. A user alias expanding to a
/// command whose first token is another user alias is followed, but only this
/// many times. trace:TASK-877 | ai:claude
const MAX_EXPANSION_HOPS: usize = 16;

/// Expand `args` (the full argv incl. `argv[0]`) against the user-alias tables,
/// IF the caller is an interactive human and `args[1]` is a user alias whose
/// name does not collide with a real subcommand. Trailing args are appended.
/// Follows alias→alias chains up to [`MAX_EXPANSION_HOPS`], then bails to avoid
/// infinite expansion (the recursion guard). Unmatched / non-human input is
/// returned unchanged. trace:TASK-877 | ai:claude
pub(crate) fn expand(args: &[String]) -> Vec<String> {
    // HARD INVARIANT: only interactive human shells expand user aliases.
    if !is_interactive_human_caller() {
        return args.to_vec();
    }
    expand_inner(args, true)
}

/// The scope-aware expansion core. `emit_cycle_warning` controls whether a
/// detected cycle prints to stderr (suppressed under unit tests). Returns the
/// rewritten argv, or the input unchanged if there is nothing to expand or a
/// cycle is detected. trace:TASK-877 | ai:claude
fn expand_inner(args: &[String], emit_cycle_warning: bool) -> Vec<String> {
    if args.len() < 2 {
        return args.to_vec();
    }
    let aliases = effective_user_aliases();
    if aliases.is_empty() {
        return args.to_vec();
    }
    let reals = real_subcommand_names();
    let lookup = |name: &str| -> Option<String> {
        aliases
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.expansion.clone())
    };

    let prog = args[0].clone();
    let mut head = args[1].clone();
    let mut tail: Vec<String> = args[2..].to_vec();

    let mut seen = std::collections::HashSet::new();
    let mut hops = 0usize;
    loop {
        // A real subcommand always wins — never expand a name clap would route.
        if reals.contains(&head) {
            break;
        }
        let Some(expansion) = lookup(&head) else {
            break;
        };
        // Recursion guard: a name seen twice, or too many hops, is a cycle.
        if !seen.insert(head.clone()) || hops >= MAX_EXPANSION_HOPS {
            if emit_cycle_warning {
                eprintln!(
                    "aida: alias `{head}` forms an expansion cycle — refusing to expand (canonical surface used)"
                );
            }
            return args.to_vec();
        }
        hops += 1;
        // Split the stored expansion into tokens; the first becomes the new
        // head, the rest prepend to the existing tail.
        let mut tokens: Vec<String> = expansion.split_whitespace().map(String::from).collect();
        if tokens.is_empty() {
            break;
        }
        head = tokens.remove(0);
        let mut new_tail = tokens;
        new_tail.append(&mut tail);
        tail = new_tail;
    }

    let mut out = Vec::with_capacity(2 + tail.len());
    out.push(prog);
    out.push(head);
    out.extend(tail);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    // BUG-697: env mutation (AIDA_TEST_HOME, AIDA_AGENT_TYPE, …) + cwd is
    // process-global; serialise on the ONE shared env lock (was a local mutex)
    // so these swaps can't race a read/swap under any other test helper.

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    // --- storage round-trip (personal + project scope) ----------------------

    #[test]
    fn add_list_remove_roundtrip_preserves_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aliases.toml");
        write(
            &path,
            "# my aliases\n[alias]\nfoo = \"list --status draft\"  # inline\n",
        );

        // Load existing.
        let rows = load_table(&path).unwrap();
        assert_eq!(
            rows,
            vec![("foo".to_string(), "list --status draft".to_string())]
        );

        // Add another via the doc-writer path (mirror `add`'s body without scope).
        let mut doc = load_doc(&path).unwrap();
        table_mut(&mut doc, "alias").insert("bar", Item::Value(Value::from("show BUG-1")));
        save_doc(&path, &doc).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("# my aliases"), "top comment preserved");
        assert!(out.contains("# inline"), "inline comment preserved");
        assert!(out.contains("bar = \"show BUG-1\""));

        // Remove one and confirm the other + comments survive.
        let mut doc = load_doc(&path).unwrap();
        assert!(doc["alias"].as_table_mut().unwrap().remove("foo").is_some());
        save_doc(&path, &doc).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(!out.contains("foo ="));
        assert!(out.contains("bar = \"show BUG-1\""));
    }

    #[test]
    fn add_refuses_real_subcommand_shadow() {
        // `list` is a real subcommand — validate_name must refuse.
        let err = validate_name("list").unwrap_err().to_string();
        assert!(err.contains("real `aida` subcommand"), "{err}");
        // `show` too.
        assert!(validate_name("show").is_err());
        // A non-colliding name passes.
        assert!(validate_name("approved").is_ok());
    }

    #[test]
    fn add_refuses_bad_names() {
        assert!(validate_name("").is_err());
        assert!(validate_name("--flag").is_err());
        assert!(validate_name("two words").is_err());
    }

    #[test]
    fn real_subcommand_names_includes_known() {
        let names = real_subcommand_names();
        assert!(names.contains("list"));
        assert!(names.contains("show"));
        assert!(names.contains("add"));
        assert!(names.contains("alias"));
    }

    // --- expansion ----------------------------------------------------------

    fn with_test_home<T>(home: &Path, f: impl FnOnce() -> T) -> T {
        let _g = crate::test_env::env_lock();
        std::env::remove_var("AIDA_AGENT_TYPE");
        std::env::remove_var("AIDA_HEADLESS");
        std::env::set_var("AIDA_TEST_HOME", home);
        let out = f();
        std::env::remove_var("AIDA_TEST_HOME");
        out
    }

    // Use the scope-aware core directly so the resolution + recursion logic is
    // testable under a non-TTY test runner (bypasses the human-caller gate).
    fn expand_core(args: &[String]) -> Vec<String> {
        expand_inner(args, false)
    }

    #[test]
    fn expansion_resolves_personal_alias_and_appends_trailing_args() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(
            &home.join("home").join(".aida").join("aliases.toml"),
            "[alias]\napproved = \"list --status approved\"\n",
        );
        with_test_home(home, || {
            // `aida approved --json` -> `aida list --status approved --json`
            let out = expand_core(&s(&["aida", "approved", "--json"]));
            assert_eq!(out, s(&["aida", "list", "--status", "approved", "--json"]));
            // A non-alias passes through.
            assert_eq!(expand_core(&s(&["aida", "status"])), s(&["aida", "status"]));
        });
    }

    #[test]
    fn project_overrides_personal_on_name_clash() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(
            &home.join("home").join(".aida").join("aliases.toml"),
            "[alias]\ndup = \"list --status draft\"\n",
        );
        // A project root with its own aliases.toml. find_project_root walks up
        // to the nearest `.git` dir; create the marker so `proj` is the root.
        let proj = dir.path().join("proj");
        std::fs::create_dir_all(proj.join(".git")).unwrap();
        write(
            &proj.join(".aida").join("aliases.toml"),
            "[alias]\ndup = \"list --status done\"\n",
        );
        let _g = crate::test_env::env_lock();
        std::env::remove_var("AIDA_AGENT_TYPE");
        std::env::remove_var("AIDA_HEADLESS");
        std::env::set_var("AIDA_TEST_HOME", home);
        let prev_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&proj).unwrap();

        let rows = effective_user_aliases();
        let dup = rows.iter().find(|r| r.name == "dup").unwrap();
        assert_eq!(dup.scope, Scope::Project, "project must win");
        assert_eq!(dup.expansion, "list --status done");

        std::env::set_current_dir(prev_cwd).unwrap();
        std::env::remove_var("AIDA_TEST_HOME");
    }

    #[test]
    fn recursion_cycle_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(
            &home.join("home").join(".aida").join("aliases.toml"),
            "[alias]\na = \"b\"\nb = \"a\"\n",
        );
        with_test_home(home, || {
            // a -> b -> a -> ... cycle: expand_core bails and returns input.
            let out = expand_core(&s(&["aida", "a"]));
            assert_eq!(out, s(&["aida", "a"]), "cycle must return input unchanged");
        });
    }

    #[test]
    fn alias_to_alias_chain_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(
            &home.join("home").join(".aida").join("aliases.toml"),
            "[alias]\nq = \"queue list\"\nmyq = \"q\"\n",
        );
        with_test_home(home, || {
            // myq -> q -> queue list
            let out = expand_core(&s(&["aida", "myq"]));
            assert_eq!(out, s(&["aida", "queue", "list"]));
        });
    }

    #[test]
    fn alias_cannot_redirect_to_shadow_a_real_command_first_token() {
        // Even if a malformed file somehow stored `list` as an alias name, the
        // real-subcommand check in the loop means we never expand it.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(
            &home.join("home").join(".aida").join("aliases.toml"),
            "[alias]\nfoo = \"bar\"\nlist = \"show BUG-1\"\n",
        );
        with_test_home(home, || {
            // `aida list` stays canonical even though a `list` alias key exists.
            let out = expand_core(&s(&["aida", "list"]));
            assert_eq!(out, s(&["aida", "list"]));
        });
    }

    // --- HARD INVARIANT: agent/headless context does not expand --------------

    #[test]
    fn agent_context_does_not_expand_but_can_still_list() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(
            &home.join("home").join(".aida").join("aliases.toml"),
            "[alias]\napproved = \"list --status approved\"\n",
        );
        let _g = crate::test_env::env_lock();
        std::env::set_var("AIDA_TEST_HOME", home);
        std::env::set_var("AIDA_AGENT_TYPE", "codex");
        std::env::remove_var("AIDA_HEADLESS");

        // The public `expand` short-circuits for a non-human caller: argv is
        // returned UNCHANGED (canonical-only).
        let out = expand(&s(&["aida", "approved"]));
        assert_eq!(
            out,
            s(&["aida", "approved"]),
            "agent context must not expand"
        );

        // …yet the alias is still discoverable to the agent (list still reads it).
        assert!(
            !is_interactive_human_caller(),
            "codex agent is not a human caller"
        );
        let rows = effective_user_aliases();
        assert!(
            rows.iter().any(|r| r.name == "approved"),
            "list still surfaces the alias"
        );

        std::env::remove_var("AIDA_AGENT_TYPE");
        std::env::remove_var("AIDA_TEST_HOME");
    }

    #[test]
    fn headless_context_does_not_expand() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        write(
            &home.join("home").join(".aida").join("aliases.toml"),
            "[alias]\napproved = \"list --status approved\"\n",
        );
        let _g = crate::test_env::env_lock();
        std::env::set_var("AIDA_TEST_HOME", home);
        std::env::remove_var("AIDA_AGENT_TYPE");
        std::env::set_var("AIDA_HEADLESS", "1");

        assert!(!is_interactive_human_caller());
        let out = expand(&s(&["aida", "approved"]));
        assert_eq!(out, s(&["aida", "approved"]));

        std::env::remove_var("AIDA_HEADLESS");
        std::env::remove_var("AIDA_TEST_HOME");
    }
}
