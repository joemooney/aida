//! BUG-1723: `.aida/config.toml` is worktree-resident and committable, so its
//! branch-local copy is POLICY-ONLY configuration. A key read from it may tune
//! policy (timeouts, thresholds, role lists, display preferences); it may NOT
//! select an executable, a shell command, or a credential — those come from
//! the running binary, from outside the worktree, or from the trusted
//! default-branch copy (`crate::trusted_config`, TASK-969).
//!
//! This guard keeps that doctrine enforced rather than remembered:
//!
//! 1. Every workspace source file whose non-test code mentions a
//!    `config.toml` literal must be inventoried in `scripts/config-trust.toml`
//!    with its channel (worktree / trusted / home / write / probe / other-file)
//!    and its exact site count — both directions, so an added, moved, or
//!    removed site forces the inventory (and so a reviewer) to look.
//! 2. Every section/key read from the branch-local copy is enumerated under
//!    `[keys.worktree.*]` and must classify as `policy` (or `location` for the
//!    frozen data-location exceptions named in this guard). A key whose NAME
//!    is authority-shaped (`bin`, `exec`, `command`, `credential`, …) is
//!    rejected no matter what class it claims.
//! 3. Code-executing keys live under `[keys.trusted.*]` and may only be read
//!    through the trusted channel.
//!
//! HONEST BOUNDARY: the site counter catches every new read location, and the
//! classifier makes every inventoried key a reviewed trust decision, but a new
//! key parsed inside an EXISTING scanner body is caught mechanically only when
//! its name trips the authority deny-list. The doctrine comment at
//! `agent_registry::Config` and the review question it poses cover that
//! remainder; this guard does not claim to.
//!
//! Both the inventory and the sources are read AT RUNTIME from
//! `CARGO_MANIFEST_DIR` — an `include_str!` copy would make the delete-the-
//! inventory mutation proof impossible (see the TASK-1555 guard, same
//! pattern).
//
// trace:BUG-1723 | ai:claude

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("aida-cli-lib always has a workspace parent")
        .to_path_buf()
}

/// Strip `#[cfg(test)]`-gated regions from a source file, returning the lines
/// that remain production code. Handles the two shapes this workspace uses:
/// a `#[cfg(test)]` attribute followed by an inline item whose body closes at
/// a `}` on the item's own indentation (the rustfmt invariant — brace
/// counting would misfire on multi-line string literals, which drain_state.rs
/// proved in practice), and a `#[cfg(test)]` attribute followed by attribute
/// lines and a `mod …;` declaration (nothing to strip beyond the declaration
/// itself).
fn production_lines(source: &str) -> Vec<(usize, String)> {
    let all: Vec<&str> = source.lines().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < all.len() {
        let line = all[i];
        if is_test_cfg(line) {
            i += 1;
            // Consume attribute lines up to the gated item's own line.
            let mut item_line: Option<&str> = None;
            while i < all.len() {
                let t = all[i].trim();
                if t.starts_with("#[") || t.is_empty() {
                    i += 1;
                    continue;
                }
                item_line = Some(all[i]);
                i += 1;
                break;
            }
            let Some(item_line) = item_line else { break };
            let trimmed_end = item_line.trim_end();
            if trimmed_end.ends_with(';') || trimmed_end.ends_with('}') {
                continue; // out-of-line `mod x;`, or single-line item
            }
            let indent: String = item_line
                .chars()
                .take_while(|c| c.is_whitespace())
                .collect();
            let close = format!("{indent}}}");
            while i < all.len() {
                if all[i].trim_end() == close {
                    // A `}` on the right column can also sit inside a
                    // multi-line raw string (TOML/JSON fixtures). A real item
                    // close is never followed by a MORE-indented line, so
                    // only accept the close when the next non-empty line is
                    // at the item's indent or shallower.
                    let next_deeper = all[i + 1..]
                        .iter()
                        .find(|l| !l.trim().is_empty())
                        .is_some_and(|l| {
                            let li: usize = l.chars().take_while(|c| c.is_whitespace()).count();
                            li > indent.len()
                        });
                    if !next_deeper {
                        i += 1;
                        break;
                    }
                }
                i += 1;
            }
            continue;
        }
        out.push((i + 1, line.to_string()));
        i += 1;
    }
    out
}

/// A `#[cfg(…)]` attribute that gates test-only code: `#[cfg(test)]` itself
/// and compound forms like `#[cfg(all(test, unix))]`. Deliberately
/// conservative — anything cfg-gated whose predicate mentions `test` is
/// treated as non-production.
fn is_test_cfg(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("#[cfg(") && t.contains("test")
}

/// The detection predicate: a production line that mentions a quoted
/// `config.toml` path component. Comment-only lines are skipped so prose may
/// name the file without becoming a counted site.
fn is_config_toml_site(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with("//") {
        return false;
    }
    line.contains("config.toml\"")
}

/// Count detected sites per file, keyed by workspace-relative path.
fn detected_sites(root: &Path, crates: &[&str]) -> BTreeMap<String, usize> {
    let mut sites: BTreeMap<String, usize> = BTreeMap::new();
    for krate in crates {
        let src = root.join(krate).join("src");
        for file in rs_files(&src) {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let count = production_lines(&text)
                .iter()
                .filter(|(_, l)| is_config_toml_site(l))
                .count();
            if count > 0 {
                let mut rel = file
                    .strip_prefix(root)
                    .expect("file is under root")
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel.starts_with("aida-cli-lib/src/lib_part") {
                    rel = "aida-cli-lib/src/lib.rs".to_string();
                }
                *sites.entry(rel).or_insert(0) += count;
            }
        }
    }
    sites
}

fn rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // src/tests/ holds test modules: never production.
            if path.file_name().is_some_and(|n| n == "tests") {
                continue;
            }
            out.extend(rs_files(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Inventory model + checks (pure over parsed TOML, so the mutation proofs can
// feed synthetic inventories without touching the real one).
// ---------------------------------------------------------------------------

const INVENTORY_RELPATH: &str = "scripts/config-trust.toml";

/// Channels a `config.toml` literal may be inventoried under.
const CHANNELS: &[&str] = &[
    "worktree",   // reads the branch-local .aida/config.toml — POLICY ONLY
    "trusted",    // reads the default-branch copy via trusted_config
    "home",       // reads ~/.aida/config.toml (outside the worktree)
    "write",      // writes/scaffolds the file, parses nothing from it
    "probe",      // existence/presence check only, no value taken
    "other-file", // a different config.toml (e.g. .codex/config.toml)
    "prose",      // the path appears in user-facing text, not as an access
];

/// Classes a branch-local (worktree) key may carry.
const WORKTREE_CLASSES: &[&str] = &["policy", "location", "grandfathered"];

/// The authority-bearing keys that were ALREADY read from the branch-local
/// copy when the BUG-1723 doctrine landed, each frozen against the bug that
/// owns its remediation. Fixing one (rerouting it through trusted_config or
/// relocating it outside the worktree) means DELETING its row here — after
/// which this guard enforces the fix forever. Adding a row is the one thing
/// this list must never absorb silently: it requires advisor signoff on the
/// named bug, which is why it lives in the guard and not in the inventory
/// file.
const GRANDFATHERED_AUTHORITY: &[(&str, &str)] = &[
    ("terminal.send_token", "BUG-1775"),
    ("preflight.guards", "BUG-1775"),
    ("behavior.permission_mode", "BUG-1775"),
    ("contained.enable", "BUG-1775"),
    ("contained.os_wrap", "BUG-1775"),
    ("contained.read_allowlist", "BUG-1775"),
    ("contained.allowed_hosts", "BUG-1775"),
    ("contained.managed_domains_only", "BUG-1775"),
    ("worktree_pool.worktree_parent", "BUG-1775"),
    ("schedule.jobs.command", "BUG-1775"),
    ("schedule.jobs.prompt", "BUG-1775"),
    ("schedule.tasks.command", "BUG-1775"),
    ("schedule.tasks.prompt", "BUG-1775"),
];

/// Classes a trusted-channel key may carry.
const TRUSTED_CLASSES: &[&str] = &["policy", "command"];

/// Key-name words that mark launch authority. A worktree key whose name
/// contains one of these (split on `_`, `-`, `.`) is rejected regardless of
/// the class the inventory claims for it.
const AUTHORITY_WORDS: &[&str] = &[
    "bin",
    "exe",
    "exec",
    "executable",
    "cmd",
    "command",
    "shell",
    "script",
    "spawn",
    "launch",
    "credential",
    "secret",
    "token",
    "password",
];

/// Key-name words that mark a filesystem location. A worktree key whose name
/// contains one of these must carry class `location` AND be individually
/// frozen in [`LOCATION_EXCEPTIONS`] — growing that list is a guard edit, not
/// an inventory edit, on purpose.
const LOCATION_WORDS: &[&str] = &["path", "dir", "file", "root"];

/// The data-location keys the branch-local copy is allowed to carry, as
/// `section.key`. These locate data AIDA reads/writes (never anything it
/// executes) and predate the BUG-1723 decision; a new entry needs advisor
/// signoff on the owning spec, which is what editing this constant forces.
/// `store_path` is the load-bearing bootstrap case: the committed file is how
/// a fresh clone finds its requirement store at all. (`_top` names keys that
/// sit at the TOML root, outside any section.)
const LOCATION_EXCEPTIONS: &[&str] = &["deployment.store_path", "_top.store_path"];

/// Keys whose NAME trips [`AUTHORITY_WORDS`] but whose value was reviewed and
/// carries no authority. Each entry keeps the word-net strong instead of
/// shrinking it: a future `*_token` or `*_command` key still fails until it
/// is either fixed or reviewed onto this list with a reason.
const NAME_FALSE_POSITIVES: &[&str] = &[
    // Presence-probed for a display note only; the acceptance-command policy
    // itself reads from ~/.aida/config.toml.
    "review.acceptance_command_allow",
    // LLM token budgets — counts, not credentials.
    "shift.daily_token_budget",
    "shift.wave_token_budget",
];

fn words(key: &str) -> Vec<String> {
    key.split(['_', '-', '.'])
        .map(|w| w.to_ascii_lowercase())
        .collect()
}

fn hits(key: &str, word_list: &[&str]) -> bool {
    words(key).iter().any(|w| word_list.contains(&w.as_str()))
}

/// Validate one worktree key. Returns the complaint, or None when it passes.
fn check_worktree_key(section: &str, key: &str, class: &str) -> Option<String> {
    let qualified = format!("{section}.{key}");
    if class == "grandfathered" {
        return if GRANDFATHERED_AUTHORITY.iter().any(|(k, _)| *k == qualified) {
            None
        } else {
            Some(format!(
                "[keys.worktree.{section}] {key}: class `grandfathered` but \
                 `{qualified}` is not frozen in GRANDFATHERED_AUTHORITY — \
                 grandfathering a NEW authority key needs advisor signoff and \
                 a tracking bug, recorded by editing the guard itself"
            ))
        };
    }
    if hits(key, AUTHORITY_WORDS) && !NAME_FALSE_POSITIVES.contains(&qualified.as_str()) {
        return Some(format!(
            "[keys.worktree.{section}] {key}: authority-shaped name — a value \
             that selects an executable, command, or credential may not be \
             read from the branch-local .aida/config.toml at all; read it \
             through trusted_config or from outside the worktree (BUG-1723)"
        ));
    }
    if !WORKTREE_CLASSES.contains(&class) {
        return Some(format!(
            "[keys.worktree.{section}] {key}: class `{class}` is not allowed \
             from the branch-local copy (allowed: {WORKTREE_CLASSES:?})"
        ));
    }
    if class == "location" || hits(key, LOCATION_WORDS) {
        if class != "location" {
            return Some(format!(
                "[keys.worktree.{section}] {key}: location-shaped name must \
                 carry class `location`, not `{class}`"
            ));
        }
        if !LOCATION_EXCEPTIONS.contains(&qualified.as_str()) {
            return Some(format!(
                "[keys.worktree.{section}] {key}: class `location` but \
                 `{qualified}` is not in LOCATION_EXCEPTIONS — a new \
                 data-location key is a guard edit with advisor signoff, \
                 not an inventory edit (BUG-1723)"
            ));
        }
    }
    None
}

struct Inventory {
    readers: BTreeMap<String, (Vec<String>, usize)>, // file -> (channels, sites)
    worktree_keys: Vec<(String, String, String)>,    // (section, key, class)
    trusted_keys: Vec<(String, String, String)>,
}

fn parse_inventory(text: &str) -> Inventory {
    let value: toml::Value = text.parse().expect("config-trust.toml must parse");
    let mut readers = BTreeMap::new();
    if let Some(tbl) = value.get("readers").and_then(|v| v.as_table()) {
        for (file, entry) in tbl {
            let channels: Vec<String> = entry
                .get("channels")
                .and_then(|v| v.as_array())
                .unwrap_or_else(|| panic!("readers.{file} needs a channels array"))
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect();
            let sites = entry
                .get("sites")
                .and_then(|v| v.as_integer())
                .unwrap_or_else(|| panic!("readers.{file} needs a site count"))
                as usize;
            readers.insert(file.clone(), (channels, sites));
        }
    }
    let keys_of = |which: &str| -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        if let Some(sections) = value
            .get("keys")
            .and_then(|k| k.get(which))
            .and_then(|v| v.as_table())
        {
            for (section, keys) in sections {
                let keys = keys
                    .as_table()
                    .unwrap_or_else(|| panic!("keys.{which}.{section} must be a table"));
                for (key, class) in keys {
                    let class = class
                        .as_str()
                        .unwrap_or_else(|| panic!("keys.{which}.{section}.{key} must be a string"))
                        .to_string();
                    out.push((section.clone(), key.clone(), class));
                }
            }
        }
        out
    };
    Inventory {
        readers,
        worktree_keys: keys_of("worktree"),
        trusted_keys: keys_of("trusted"),
    }
}

/// All complaints for (detected sites × inventory). Pure, so the mutation
/// proofs below can drive it with synthetic inputs.
fn check(detected: &BTreeMap<String, usize>, inventory: &Inventory) -> Vec<String> {
    let mut complaints = Vec::new();
    for (file, count) in detected {
        match inventory.readers.get(file) {
            None => complaints.push(format!(
                "{file}: {count} config.toml site(s) in production code but no \
                 [readers] entry in {INVENTORY_RELPATH} — classify the read \
                 (worktree reads are policy-only; see BUG-1723)"
            )),
            Some((channels, expected)) => {
                for channel in channels {
                    if !CHANNELS.contains(&channel.as_str()) {
                        complaints.push(format!(
                            "{file}: unknown channel `{channel}` (allowed: {CHANNELS:?})"
                        ));
                    }
                }
                if channels.is_empty() {
                    complaints.push(format!("{file}: empty channels array"));
                }
                if expected != count {
                    complaints.push(format!(
                        "{file}: {count} config.toml site(s) found, inventory \
                         says {expected} — a site was added, moved, or removed; \
                         re-review and update {INVENTORY_RELPATH}"
                    ));
                }
            }
        }
    }
    for file in inventory.readers.keys() {
        if !detected.contains_key(file) {
            complaints.push(format!(
                "{file}: inventoried in {INVENTORY_RELPATH} but no config.toml \
                 site found in its production code — stale entry, remove or fix"
            ));
        }
    }
    for (section, key, class) in &inventory.worktree_keys {
        if let Some(c) = check_worktree_key(section, key, class) {
            complaints.push(c);
        }
    }
    for (section, key, class) in &inventory.trusted_keys {
        if !TRUSTED_CLASSES.contains(&class.as_str()) {
            complaints.push(format!(
                "[keys.trusted.{section}] {key}: class `{class}` not allowed \
                 (allowed: {TRUSTED_CLASSES:?})"
            ));
        }
    }
    // The freeze list may not outlive the inventory rows it blesses: a fixed
    // key whose grandfather row is forgotten would silently re-authorize the
    // worktree copy if the key ever came back.
    let grandfathered_in_inventory: BTreeSet<String> = inventory
        .worktree_keys
        .iter()
        .filter(|(_, _, class)| class == "grandfathered")
        .map(|(section, key, _)| format!("{section}.{key}"))
        .collect();
    for (qualified, bug) in GRANDFATHERED_AUTHORITY {
        if !grandfathered_in_inventory.contains(*qualified) {
            complaints.push(format!(
                "GRANDFATHERED_AUTHORITY entry `{qualified}` ({bug}) has no \
                 matching grandfathered inventory key — the key was fixed or \
                 renamed, so delete its row from the guard"
            ));
        }
    }
    complaints
}

// ---------------------------------------------------------------------------
// The real check.
// ---------------------------------------------------------------------------

const SCANNED_CRATES: &[&str] = &[
    "aida-core",
    "aida-cli-lib",
    "aida-cli",
    "aida-tui",
    "aida-mcp",
    "aida-server",
];

#[test]
fn every_config_toml_site_is_inventoried_and_every_worktree_key_is_policy() {
    let root = workspace_root();
    let inv_path = root.join(INVENTORY_RELPATH);
    let text = std::fs::read_to_string(&inv_path).unwrap_or_else(|err| {
        panic!(
            "{} must exist: {err}\n\
             It is the enumerated trust classification of every config.toml \
             read site (BUG-1723). Without it the worktree copy's policy-only \
             doctrine is unenforced, so its absence is a failure, not a pass.",
            inv_path.display()
        )
    });
    let inventory = parse_inventory(&text);
    let detected = detected_sites(&root, SCANNED_CRATES);
    assert!(
        !detected.is_empty(),
        "the detector found no config.toml sites at all — the scan idiom has \
         drifted from the codebase and this guard is checking nothing"
    );
    let complaints = check(&detected, &inventory);
    assert!(
        complaints.is_empty(),
        "config.toml trust guard failed:\n  {}",
        complaints.join("\n  ")
    );
}

/// AC3 control: the three `[agent_registry]` keys that exist today classify
/// clean, exactly as inventoried.
#[test]
fn todays_agent_registry_keys_pass_the_classifier() {
    for key in ["busy_threshold_secs", "work_grace_secs", "singleton_roles"] {
        assert_eq!(check_worktree_key("agent_registry", key, "policy"), None);
    }
}

// ---------------------------------------------------------------------------
// Mutation proofs (AC2): the guard must FAIL on an executable/path key and on
// an uninventoried or stale read site. Each case drives the pure core with a
// synthetic input so the proof lives here permanently instead of being a
// one-off manual edit.
// ---------------------------------------------------------------------------

#[test]
fn guard_rejects_an_executable_key_even_when_labelled_policy() {
    let complaint = check_worktree_key("agents", "aida_bin", "policy");
    assert!(
        complaint.is_some_and(|c| c.contains("authority-shaped")),
        "an executable-selecting key must be rejected by NAME, before any \
         class label is believed"
    );
}

#[test]
fn guard_rejects_a_command_class_and_a_path_key_from_the_worktree() {
    assert!(
        check_worktree_key("pr-rebase", "smoke_check", "command").is_some(),
        "class `command` must never be accepted from the branch-local copy"
    );
    assert!(
        check_worktree_key("hooks", "handler_path", "policy").is_some(),
        "a path-shaped key must not pass as plain policy"
    );
    assert!(
        check_worktree_key("hooks", "handler_path", "location").is_some(),
        "a location key outside LOCATION_EXCEPTIONS must still fail — the \
         exception list is frozen in this guard on purpose"
    );
}

#[test]
fn grandfathering_is_frozen_to_the_guards_own_list() {
    // A fixed command key is now rejected by the same authority-name rule as
    // every other non-grandfathered worktree key.
    assert!(check_worktree_key("notify", "command", "policy")
        .is_some_and(|c| c.contains("authority-shaped")));
    // …but a NEW key cannot ride in on the class name alone.
    assert!(
        check_worktree_key("deploy", "post_merge_hook", "grandfathered")
            .is_some_and(|c| c.contains("GRANDFATHERED_AUTHORITY")),
        "grandfathering a key the guard does not freeze must fail"
    );
}

#[test]
fn guard_fails_on_an_uninventoried_site_and_on_a_stale_entry() {
    let inventory = parse_inventory(
        r#"
        [readers."a/src/known.rs"]
        channels = ["worktree"]
        sites = 1
        [readers."a/src/ghost.rs"]
        channels = ["probe"]
        sites = 2
        "#,
    );
    let mut detected = BTreeMap::new();
    detected.insert("a/src/known.rs".to_string(), 1usize);
    detected.insert("a/src/new_reader.rs".to_string(), 1usize);
    let complaints = check(&detected, &inventory);
    assert!(
        complaints.iter().any(|c| c.contains("new_reader.rs")),
        "a read site with no inventory entry must fail: {complaints:?}"
    );
    assert!(
        complaints.iter().any(|c| c.contains("ghost.rs")),
        "an inventory entry with no read site must fail (both directions): \
         {complaints:?}"
    );
}

#[test]
fn guard_fails_on_a_site_count_change() {
    let inventory = parse_inventory(
        r#"
        [readers."a/src/known.rs"]
        channels = ["worktree"]
        sites = 1
        "#,
    );
    let mut detected = BTreeMap::new();
    detected.insert("a/src/known.rs".to_string(), 2usize);
    let complaints = check(&detected, &inventory);
    assert!(
        complaints
            .iter()
            .any(|c| c.contains("added, moved, or removed")),
        "a second read site in a known file must force re-review: {complaints:?}"
    );
}

#[test]
fn detector_sees_through_test_regions_and_comments() {
    let source = r#"
fn real() {
    let p = root.join(".aida").join("config.toml");
}
// a comment naming "config.toml" is not a site
#[cfg(test)]
mod tests {
    fn fixture() {
        let p = tmp.join("config.toml");
    }
}
"#;
    let hits: Vec<_> = production_lines(source)
        .into_iter()
        .filter(|(_, l)| is_config_toml_site(l))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "exactly the production join must count — not the comment, not the \
         test fixture: {hits:?}"
    );
}
