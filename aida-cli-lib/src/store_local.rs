//! Machine-local, per-clone store topology: `~/.aida/store-local.toml`.
//!
//! Some clones must not reach every hub. A work clone may push the store to
//! gitlab only and never to GitHub, while the home clone reaches both. The
//! tracked `.aida/config.toml` is shared by every clone, so it cannot say
//! which clone is which. This layer is untracked and keyed by the clone's
//! primary checkout, so every seat worktree of one clone reads the same
//! entry:
//!
//! ```toml
//! [repo."/home/me/src/project"]
//! bridge = true            # this clone union-merges every hub on a schedule
//! bridge_every = "1h"      # optional cadence of the bridge job
//! push_hubs = ["gitlab"]   # the only remotes this clone pushes the store to
//! ```
//!
//! - `bridge = true` makes the schedule registry run `remote reconcile
//!   --execute` (never `--yes`) from this clone only. A tracked or
//!   machine-global schedule job cannot run a reconcile.
//! - `push_hubs` limits every store push (`db sync --push`, `aida push`,
//!   per-write auto-push, `remote mirror-sync`, `remote reconcile`). When
//!   `origin` is not listed, the store goes to the listed hubs instead.
//!   An unreadable layer refuses every store push: it may be the file that
//!   says "never push to origin".
//!
//! trace:STORY-B6B1278890774EF993D2EBC327862470-097 | ai:claude

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The layer's file name under `<AIDA_HOME|home>/.aida/`.
pub(crate) const LOCAL_LAYER_FILE: &str = "store-local.toml";
/// The substrate job a bridge clone runs.
pub(crate) const BRIDGE_JOB: &str = "store-bridge-reconcile";
/// The seat job that carries a failed bridge run to the advisor.
pub(crate) const BRIDGE_ROUTE_JOB: &str = "store-bridge-reconcile-route";
/// The bridge job's cadence when the layer does not set `bridge_every`.
pub(crate) const DEFAULT_BRIDGE_EVERY: &str = "1h";
/// The scheduled command the bridge job runs. `--yes` is never added, so
/// identity-bearing content stops the reconcile instead of being published.
pub(crate) const BRIDGE_COMMAND: &str = "remote reconcile --execute";
/// The marker of the hand-installed stopgap crontab entry that the bridge
/// job replaces.
pub(crate) const STOPGAP_CRON_MARKER: &str = "# aida-store-reconcile";

/// This clone's entry in the layer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StoreLocal {
    pub(crate) bridge: bool,
    pub(crate) bridge_every: Option<String>,
    /// `None` = no restriction (every configured hub may receive the store).
    pub(crate) push_hubs: Option<Vec<String>>,
}

/// Which remotes this clone may push the store to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PushHubs {
    /// No `push_hubs` entry: the default origin-then-mirrors behaviour.
    Unrestricted,
    /// Only these remotes, in this order.
    Only(Vec<String>),
    /// The layer exists but cannot be read; every store push is refused.
    Unreadable(String),
}

impl PushHubs {
    pub(crate) fn allows(&self, remote: &str) -> bool {
        match self {
            PushHubs::Unrestricted => true,
            PushHubs::Only(hubs) => hubs.iter().any(|h| h == remote),
            PushHubs::Unreadable(_) => false,
        }
    }

    /// Why a push to `remote` is skipped, for a warning line.
    pub(crate) fn refusal(&self, remote: &str) -> String {
        match self {
            PushHubs::Unrestricted => String::new(),
            PushHubs::Only(hubs) => format!(
                "`{remote}` is not one of this clone's push hubs ({}) — skipped",
                hubs.join(", ")
            ),
            PushHubs::Unreadable(err) => {
                format!("store push to `{remote}` refused: {err}")
            }
        }
    }
}

/// `<AIDA_HOME|home>/.aida/store-local.toml`.
pub(crate) fn layer_path() -> Option<PathBuf> {
    crate::maintenance_schedule::global_home().map(|h| h.join(".aida").join(LOCAL_LAYER_FILE))
}

/// The key a clone is filed under: its primary checkout, canonicalized, so a
/// linked worktree or seat clone resolves to the clone it belongs to.
pub(crate) fn clone_key(project_root: &Path) -> String {
    let main = crate::worktree_gc::main_worktree_root_from(project_root);
    main.canonicalize().unwrap_or(main).display().to_string()
}

/// Is `project_root` the clone's primary checkout (not a worktree of it)?
pub(crate) fn is_primary_checkout(project_root: &Path) -> bool {
    let here = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    here.display().to_string() == clone_key(project_root)
}

/// Parse `key`'s entry out of a layer body. A missing entry is the default.
pub(crate) fn parse_entry(body: &str, key: &str) -> Result<StoreLocal> {
    let doc: toml::Value = toml::from_str(body).context("not valid TOML")?;
    let Some(entry) = doc.get("repo").and_then(|r| r.get(key)) else {
        return Ok(StoreLocal::default());
    };
    let bridge = match entry.get("bridge") {
        None => false,
        Some(v) => v
            .as_bool()
            .with_context(|| format!("[repo.\"{key}\"] bridge must be true or false"))?,
    };
    let bridge_every = match entry.get("bridge_every") {
        None => None,
        Some(v) => Some(
            v.as_str()
                .with_context(|| format!("[repo.\"{key}\"] bridge_every must be a string"))?
                .to_string(),
        ),
    };
    let push_hubs = match entry.get("push_hubs") {
        None => None,
        Some(toml::Value::Array(arr)) => Some(
            arr.iter()
                .map(|v| {
                    v.as_str().map(str::to_string).with_context(|| {
                        format!("[repo.\"{key}\"] push_hubs must list remote names")
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        ),
        Some(_) => anyhow::bail!("[repo.\"{key}\"] push_hubs must be an array of remote names"),
    };
    Ok(StoreLocal {
        bridge,
        bridge_every,
        push_hubs,
    })
}

/// Read `key`'s entry from the layer at `path`. A missing file is the default.
pub(crate) fn load_from(path: &Path, key: &str) -> Result<StoreLocal> {
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(StoreLocal::default()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    parse_entry(&body, key).with_context(|| format!("{} is malformed", path.display()))
}

/// This clone's entry. A missing layer costs no git call.
pub(crate) fn load(project_root: &Path) -> Result<StoreLocal> {
    let Some(path) = layer_path() else {
        return Ok(StoreLocal::default());
    };
    if !path.exists() {
        return Ok(StoreLocal::default());
    }
    load_from(&path, &clone_key(project_root))
}

/// This clone's push policy for the store.
pub(crate) fn push_hubs(project_root: &Path) -> PushHubs {
    match load(project_root) {
        Ok(StoreLocal {
            push_hubs: Some(hubs),
            ..
        }) => PushHubs::Only(hubs),
        Ok(_) => PushHubs::Unrestricted,
        Err(e) => PushHubs::Unreadable(format!("{e:#}")),
    }
}

/// The [`aida_core::git_ops::StorePushGate`] the CLI registers: refuse a
/// store push to a remote this clone's push hubs exclude. `repo` may be the
/// store worktree; it resolves to the same clone entry.
pub(crate) fn store_push_gate(repo: &Path, remote: &str) -> Option<String> {
    let policy = push_hubs(repo);
    (!policy.allows(remote)).then(|| policy.refusal(remote))
}

/// The hubs a store push goes to instead of `origin` when this clone's push
/// hubs exclude it: the declared hubs that exist as remotes here. `None`
/// when `origin` is allowed (the normal origin-then-mirrors path applies).
pub(crate) fn redirected_store_hubs(
    policy: &PushHubs,
    has_remote: impl Fn(&str) -> bool,
) -> Option<Vec<String>> {
    match policy {
        PushHubs::Unrestricted => None,
        PushHubs::Only(hubs) if hubs.iter().any(|h| h == "origin") => None,
        PushHubs::Only(hubs) => Some(hubs.iter().filter(|h| has_remote(h)).cloned().collect()),
        PushHubs::Unreadable(_) => Some(Vec::new()),
    }
}

/// Push store commit `sha` to each of `hubs` (the redirect when `origin` is
/// not a push hub). Returns `(pushed, failures)`; a rejected hub has diverged
/// and is left for the hub bridge.
pub(crate) fn push_store_to_hubs(
    store: &Path,
    branch: &str,
    sha: &str,
    hubs: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut pushed = Vec::new();
    let mut failures = Vec::new();
    for hub in hubs {
        match aida_core::git_ops::push_exact(store, hub, sha, branch) {
            Ok(true) => pushed.push(hub.clone()),
            Ok(false) => failures.push(format!(
                "{hub}: rejected (diverged) — `aida db sync --pull` from it, or let the hub bridge reconcile"
            )),
            Err(e) => failures.push(format!("{hub}: {e:#}")),
        }
    }
    (pushed, failures)
}

/// Write `bridge` for this clone into the layer at `path`.
pub(crate) fn write_bridge(path: &Path, key: &str, bridge: bool) -> Result<()> {
    edit_entry(path, key, |t| {
        t.insert("bridge", toml_edit::value(bridge));
    })
}

/// Write (or with `None`, clear) `push_hubs` for this clone.
pub(crate) fn write_push_hubs(path: &Path, key: &str, hubs: Option<&[String]>) -> Result<()> {
    edit_entry(path, key, |t| match hubs {
        Some(hubs) => {
            let mut arr = toml_edit::Array::new();
            for h in hubs {
                arr.push(h.as_str());
            }
            t.insert("push_hubs", toml_edit::value(arr));
        }
        None => {
            t.remove("push_hubs");
        }
    })
}

fn edit_entry(path: &Path, key: &str, edit: impl FnOnce(&mut toml_edit::Table)) -> Result<()> {
    let mut doc = crate::config_edit::load_doc(path)?;
    if !doc.contains_table("repo") {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        doc.insert("repo", toml_edit::Item::Table(t));
    }
    let repos = doc["repo"]
        .as_table_mut()
        .context("[repo] in the store-local layer is not a table")?;
    if !repos.contains_key(key) {
        repos.insert(key, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let table = repos[key]
        .as_table_mut()
        .context("the repo entry in the store-local layer is not a table")?;
    edit(table);
    crate::config_edit::save_doc(path, &doc)
}

/// Is `line` the stopgap reconcile entry (or its marker comment)?
fn is_stopgap_line(line: &str) -> bool {
    line.contains(STOPGAP_CRON_MARKER)
        || (line.contains("remote reconcile") && line.contains("store-reconcile"))
}

/// The crontab with the stopgap reconcile entry removed, or `None` when it
/// carries no such entry. Every other line is kept byte for byte.
pub(crate) fn crontab_without_stopgap(existing: &str) -> Option<String> {
    if !existing.lines().any(is_stopgap_line) {
        return None;
    }
    let mut out: String = existing
        .lines()
        .filter(|l| !is_stopgap_line(l))
        .collect::<Vec<_>>()
        .join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    Some(out)
}

/// Remove the stopgap entry from the user's crontab. Returns whether a line
/// was removed.
pub(crate) fn remove_stopgap_cron() -> Result<bool> {
    let Some(existing) = crate::maintenance_schedule::read_crontab()? else {
        return Ok(false);
    };
    match crontab_without_stopgap(&existing) {
        Some(body) => {
            crate::maintenance_schedule::write_crontab(&body)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// `aida remote bridge` — show or change whether this clone is the hub
/// bridge. Enabling also removes the hand-installed stopgap crontab entry.
pub(crate) fn handle_remote_bridge(project_root: &Path, enable: bool, disable: bool) -> Result<()> {
    let path =
        layer_path().context("cannot resolve the home directory for the store-local layer")?;
    let key = clone_key(project_root);
    if enable || disable {
        write_bridge(&path, &key, enable)?;
    }
    let entry = load_from(&path, &key)?;
    let check = crate::glyph(crate::glyphs::Glyph::Check);
    println!("Clone      {key}");
    println!("Layer      {}", path.display());
    println!(
        "Bridge     {}",
        if entry.bridge {
            format!(
                "yes — `{BRIDGE_JOB}` runs `aida {BRIDGE_COMMAND}` every {}",
                entry
                    .bridge_every
                    .as_deref()
                    .unwrap_or(DEFAULT_BRIDGE_EVERY)
            )
        } else {
            "no".to_string()
        }
    );
    println!(
        "Push hubs  {}",
        match &entry.push_hubs {
            Some(h) if h.is_empty() => "none (store pushes are disabled)".to_string(),
            Some(h) => h.join(", "),
            None => "every hub (unrestricted)".to_string(),
        }
    );
    if enable {
        match remove_stopgap_cron() {
            Ok(true) => println!(
                "{check} removed the stopgap `{STOPGAP_CRON_MARKER}` crontab entry — the bridge job replaces it"
            ),
            Ok(false) => {}
            Err(e) => eprintln!(
                "{} could not check the crontab for the stopgap entry: {e:#}",
                crate::glyph(crate::glyphs::Glyph::Warning)
            ),
        }
        println!(
            "The job runs on the schedule tick; check it is installed with `aida schedule status`."
        );
        println!(
            "Identity-bearing content stops the reconcile and routes to the advisor; it is never published unattended."
        );
    }
    Ok(())
}

/// `aida remote push-hubs` — show or set the remotes this clone pushes the
/// store to.
pub(crate) fn handle_remote_push_hubs(
    project_root: &Path,
    hubs: &[String],
    all: bool,
) -> Result<()> {
    let path =
        layer_path().context("cannot resolve the home directory for the store-local layer")?;
    let key = clone_key(project_root);
    if all {
        write_push_hubs(&path, &key, None)?;
    } else if !hubs.is_empty() {
        for h in hubs {
            if !aida_core::git_ops::has_remote(project_root, h) {
                eprintln!(
                    "{} `{h}` is not a git remote in this clone yet — store pushes to it will be skipped until it is",
                    crate::glyph(crate::glyphs::Glyph::Warning)
                );
            }
        }
        write_push_hubs(&path, &key, Some(hubs))?;
    }
    let entry = load_from(&path, &key)?;
    println!("Clone      {key}");
    println!("Layer      {}", path.display());
    match &entry.push_hubs {
        None => println!("Push hubs  every hub (unrestricted)"),
        Some(h) => {
            println!("Push hubs  {}", h.join(", "));
            if !h.iter().any(|r| r == "origin") {
                println!(
                    "           store pushes skip `origin` and go to the hubs above; a hub bridge clone (`aida remote bridge --enable`) carries them to every hub"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "/home/me/src/project";

    #[test]
    fn missing_entry_is_unrestricted_and_not_a_bridge() {
        let entry = parse_entry("[repo.\"/elsewhere\"]\nbridge = true\n", KEY).unwrap();
        assert_eq!(entry, StoreLocal::default());
    }

    #[test]
    fn entry_parses_bridge_cadence_and_push_hubs() {
        let body = format!(
            "[repo.\"{KEY}\"]\nbridge = true\nbridge_every = \"30m\"\npush_hubs = [\"gitlab\"]\n"
        );
        let entry = parse_entry(&body, KEY).unwrap();
        assert!(entry.bridge);
        assert_eq!(entry.bridge_every.as_deref(), Some("30m"));
        assert_eq!(entry.push_hubs, Some(vec!["gitlab".to_string()]));
    }

    #[test]
    fn malformed_entry_is_an_error_not_a_default() {
        let body = format!("[repo.\"{KEY}\"]\npush_hubs = \"gitlab\"\n");
        assert!(parse_entry(&body, KEY).is_err());
        assert!(parse_entry("not = [toml", KEY).is_err());
    }

    #[test]
    fn push_hubs_policy_gates_origin() {
        let only = PushHubs::Only(vec!["gitlab".into()]);
        assert!(only.allows("gitlab"));
        assert!(!only.allows("origin"));
        assert!(only
            .refusal("origin")
            .contains("not one of this clone's push hubs"));
        assert!(PushHubs::Unrestricted.allows("origin"));
        // An unreadable layer may be the one that forbids origin: fail closed.
        let broken = PushHubs::Unreadable("bad".into());
        assert!(!broken.allows("origin"));
        assert!(!broken.allows("gitlab"));
    }

    #[test]
    fn origin_excluded_redirects_to_declared_hubs_that_exist() {
        let has = |r: &str| r == "gitlab" || r == "origin";
        assert_eq!(redirected_store_hubs(&PushHubs::Unrestricted, has), None);
        let with_origin = PushHubs::Only(vec!["origin".into(), "gitlab".into()]);
        assert_eq!(redirected_store_hubs(&with_origin, has), None);
        let gitlab_only = PushHubs::Only(vec!["gitlab".into(), "missing".into()]);
        assert_eq!(
            redirected_store_hubs(&gitlab_only, has),
            Some(vec!["gitlab".to_string()])
        );
        // Unreadable: redirect to nowhere — nothing is pushed.
        assert_eq!(
            redirected_store_hubs(&PushHubs::Unreadable("x".into()), has),
            Some(Vec::new())
        );
    }

    #[test]
    fn writes_round_trip_and_keep_other_clones() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LOCAL_LAYER_FILE);
        std::fs::write(&path, "[repo.\"/other\"]\nbridge = true\n").unwrap();
        write_bridge(&path, KEY, true).unwrap();
        write_push_hubs(&path, KEY, Some(&["gitlab".to_string()])).unwrap();
        let entry = load_from(&path, KEY).unwrap();
        assert!(entry.bridge);
        assert_eq!(entry.push_hubs, Some(vec!["gitlab".to_string()]));
        write_push_hubs(&path, KEY, None).unwrap();
        write_bridge(&path, KEY, false).unwrap();
        assert_eq!(load_from(&path, KEY).unwrap(), StoreLocal::default());
        assert!(load_from(&path, "/other").unwrap().bridge);
    }

    #[test]
    fn missing_layer_file_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let entry = load_from(&dir.path().join(LOCAL_LAYER_FILE), KEY).unwrap();
        assert_eq!(entry, StoreLocal::default());
    }

    #[test]
    fn stopgap_cron_lines_are_removed_and_others_kept() {
        let tab = "MAILTO=\"\"\n\
                   */15 * * * * cd /repo && aida schedule tick # aida-schedule-tick:/repo\n\
                   # aida-store-reconcile\n\
                   0 * * * * cd /home/me/src/project && flock -n ~/.aida/store-reconcile.lock aida remote reconcile --execute >> ~/.aida/store-reconcile.log 2>&1\n\
                   30 2 * * * backup.sh\n";
        let out = crontab_without_stopgap(tab).unwrap();
        assert_eq!(
            out,
            "MAILTO=\"\"\n\
             */15 * * * * cd /repo && aida schedule tick # aida-schedule-tick:/repo\n\
             30 2 * * * backup.sh\n"
        );
        // A trailing marker on the entry itself.
        let inline = "0 * * * * aida remote reconcile --execute # aida-store-reconcile\nkeep.sh\n";
        assert_eq!(crontab_without_stopgap(inline).unwrap(), "keep.sh\n");
        assert!(crontab_without_stopgap("30 2 * * * backup.sh\n").is_none());
    }

    fn git(cwd: &Path, args: &[&str]) -> String {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(cwd)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Criterion 3 end to end: a clone whose push hubs are `gitlab` sends
    /// its store write to gitlab and leaves origin untouched.
    #[test]
    fn gitlab_only_clone_pushes_the_store_to_gitlab_and_never_origin() {
        let home = tempfile::tempdir().unwrap();
        let _guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", home.path());
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin.git");
        let gitlab = root.path().join("gitlab.git");
        for hub in [&origin, &gitlab] {
            std::fs::create_dir_all(hub).unwrap();
            git(hub, &["init", "-q", "--bare"]);
        }
        let store = root.path().join("store");
        std::fs::create_dir_all(&store).unwrap();
        git(&store, &["init", "-q", "-b", "aida-store"]);
        git(&store, &["commit", "-q", "--allow-empty", "-m", "seed"]);
        git(
            &store,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(
            &store,
            &["remote", "add", "gitlab", gitlab.to_str().unwrap()],
        );
        git(&store, &["push", "-q", "origin", "aida-store"]);
        git(&store, &["push", "-q", "gitlab", "aida-store"]);
        let seed = git(&store, &["rev-parse", "HEAD"]);

        let layer = home.path().join(".aida").join(LOCAL_LAYER_FILE);
        write_push_hubs(&layer, &clone_key(&store), Some(&["gitlab".to_string()])).unwrap();
        assert!(store_push_gate(&store, "origin").is_some());
        assert!(store_push_gate(&store, "gitlab").is_none());

        std::fs::write(store.join("spec.yaml"), "id: X\n").unwrap();
        crate::store_push_pull::auto_push_store_best_effort(&store, "test write");
        let head = git(&store, &["rev-parse", "HEAD"]);
        assert_ne!(head, seed, "the pending write was committed");
        assert_eq!(git(&gitlab, &["rev-parse", "aida-store"]), head);
        assert_eq!(git(&origin, &["rev-parse", "aida-store"]), seed);
    }

    #[test]
    fn worktree_of_a_clone_resolves_to_its_primary_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let wt = dir.path().join("wt");
        git(&main, &["worktree", "add", "-q", wt.to_str().unwrap()]);
        let want = main.canonicalize().unwrap().display().to_string();
        assert_eq!(clone_key(&wt), want);
        assert_eq!(clone_key(&main), want);
        assert!(is_primary_checkout(&main));
        assert!(!is_primary_checkout(&wt));
    }
}
