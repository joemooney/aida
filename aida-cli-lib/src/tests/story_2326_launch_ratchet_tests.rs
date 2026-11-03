//! STORY-2326 ratchet (TASK-1749 C2 style: a token scanner, no parser
//! dependency). Outside `launch/` and `aida_core::launch_policy`, production
//! source may not construct a vendor child or write a protected launch key,
//! except at the exact sites in [`LEDGER`]. The ledger may only shrink: each
//! later slice converts sites and lowers its counts, and it reaches zero
//! when the last slice lands. A new site, in any crate, fails here.
// trace:STORY-2326 | ai:claude

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const CRATES: &[&str] = &["aida-core", "aida-cli-lib", "aida-tui"];

/// Modules that are the launcher itself.
const LAUNCHER: &[&str] = &["aida-cli-lib/src/launch/", "aida-core/src/launch_policy.rs"];

/// (file, markers, why it is not converted yet). Exact counts; the total
/// may only fall (see [`CEILING`]).
const LEDGER: &[(&str, usize, &str)] = &[
    ("aida-cli-lib/src/agent_launch.rs", 5, "slice B: Codex grant/finalize writes; slice D: the harness version probe"),
    ("aida-cli-lib/src/autopilot_intake.rs", 3, "slice B: autopilot intake"),
    ("aida-cli-lib/src/claude_agents.rs", 1, "slice D: vendor query (agents list)"),
    ("aida-cli-lib/src/compete_cmd.rs", 3, "slice B: compete arms"),
    ("aida-cli-lib/src/containment/supervisor.rs", 1, "STORY-2238: the containment supervisor, a permitted constructor module"),
    ("aida-cli-lib/src/dispatch.rs", 2, "slice B: dispatch implementer launch (Q5)"),
    ("aida-cli-lib/src/doctor_cmd.rs", 1, "slice D: vendor query (codex version)"),
    ("aida-cli-lib/src/init_cmd.rs", 4, "slice D: vendor queries (MCP list/add)"),
    ("aida-cli-lib/src/lane_env.rs", 1, "the in-process lane seat correction (Q2), not a spawn"),
    ("aida-cli-lib/src/orchestrator_leases.rs", 1, "slice B: control child"),
    ("aida-cli-lib/src/phase_driver.rs", 5, "slice B: phase advisor and auto-punt control child"),
    ("aida-cli-lib/src/phase_driver_trait.rs", 2, "slice B: CI-fix launch"),
    ("aida-cli-lib/src/questions.rs", 1, "slice B: questions clarify"),
    ("aida-cli-lib/src/queue_cmd.rs", 1, "slice A2/B: the process-wide AIDA_SESSION_ROLE export (resume and Codex exec still read it)"),
    ("aida-cli-lib/src/session.rs", 41, "slice A2: the three Claude resume paths; slice B: Codex session/exec/resume and the vendor resume; the shared seat-env helper; the contained export (its aida control children convert in slice B)"),
    ("aida-cli-lib/src/zen_drive_root.rs", 2, "slice B: control child"),
    ("aida-tui/src/redesign/mod.rs", 12, "slice B: control_env"),
];

/// The slice-A total. A later slice lowers it; it is zero after slice D.
const CEILING: usize = 86;

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "tests") {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Production code: comment lines dropped and every `#[cfg(test)]` module
/// (inline body or `#[path]` declaration) skipped.
fn production(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        let code = lines[i].trim_start();
        if code.starts_with("#[cfg(test)]") {
            // Skip attributes up to the item, then the item itself.
            let mut j = i + 1;
            while lines
                .get(j)
                .is_some_and(|l| l.trim_start().starts_with("#["))
            {
                j += 1;
            }
            if lines.get(j).is_some_and(|l| {
                let l = l.trim_start();
                l.starts_with("mod ")
                    || l.starts_with("pub(crate) mod ")
                    || l.starts_with("pub mod ")
            }) {
                let mut depth = 0i32;
                let mut opened = false;
                while j < lines.len() {
                    for c in lines[j].chars() {
                        match c {
                            '{' => {
                                depth += 1;
                                opened = true;
                            }
                            '}' => depth -= 1,
                            _ => {}
                        }
                    }
                    if !opened && lines[j].trim_end().ends_with(';') {
                        break;
                    }
                    if opened && depth <= 0 {
                        break;
                    }
                    j += 1;
                }
                i = j + 1;
                continue;
            }
        }
        if !code.starts_with("//") {
            out.push_str(code);
            out.push('\n');
        }
        i += 1;
    }
    out
}

const VENDORS: &[&str] = &[
    "claude",
    "codex",
    "agy",
    "antigravity",
    "gemini",
    "opencode",
];
const PROTECTED: &[&str] = &[
    "\"AIDA_SESSION_ROLE\"",
    "\"AIDA_SESSION_GRANT\"",
    "\"AIDA_CONTAINED\"",
    "\"AIDA_CONTAINMENT_POSTURE\"",
    "GRANT_ENV",
    "ROLE_ENV",
    "CONTAINED_ENV",
    "POSTURE_ENV",
];

fn is_protected(arg: &str) -> bool {
    let arg = arg.trim_start_matches('&');
    PROTECTED
        .iter()
        .any(|key| arg == *key || (!key.starts_with('"') && arg.ends_with(&format!("::{key}"))))
}

/// The text of one call's arguments, `rest` starting after its `(`.
fn call_args(rest: &str) -> &str {
    let mut depth = 0usize;
    for (at, c) in rest.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => return &rest[..at],
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    rest
}

/// Launch markers in whitespace-normalized production source.
fn markers(source: &str) -> usize {
    let normalized: String = production(source).split_whitespace().collect();
    // `Command` and any `use …Command as X` alias (std or tokio).
    let mut ctors = vec!["Command".to_string()];
    for alias in normalized.split("Commandas").skip(1) {
        let name: String = alias
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            ctors.push(name);
        }
    }
    let mut n = 0;
    for ctor in &ctors {
        for call in normalized.split(&format!("{ctor}::new(")).skip(1) {
            let arg = call_args(call);
            let literal = VENDORS
                .iter()
                .any(|v| arg == format!("\"{v}\"") || arg == format!("&\"{v}\""));
            let dynamic = arg.contains("resolve_agent_program")
                || arg.contains(".program()")
                || arg.contains("plan.program");
            if literal || dynamic {
                n += 1;
            }
        }
    }
    for writer in [".env(", ".env_remove(", "set_var(", "remove_var("] {
        n += normalized
            .split(writer)
            .skip(1)
            .filter(|call| is_protected(call.split([',', ')']).next().unwrap_or_default()))
            .count();
    }
    n += normalized
        .split(".envs(")
        .skip(1)
        .filter(|call| {
            call_args(call)
                .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':' || c == '"'))
                .any(is_protected)
        })
        .count();
    n
}

#[test]
fn launch_sites_outside_the_launcher_match_the_ledger() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut found = BTreeMap::new();
    for krate in CRATES {
        let mut files = Vec::new();
        rust_files(&root.join(krate).join("src"), &mut files);
        for path in files {
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            // `tests.rs` / `*_tests.rs` files are cfg(test) modules of their parent.
            let name = path.file_name().unwrap().to_string_lossy();
            if LAUNCHER.iter().any(|l| rel.starts_with(l))
                || name == "tests.rs"
                || name.ends_with("_tests.rs")
            {
                continue;
            }
            let n = markers(&std::fs::read_to_string(&path).unwrap());
            if n > 0 {
                found.insert(rel, n);
            }
        }
    }
    let ledger: BTreeMap<String, usize> =
        LEDGER.iter().map(|(f, n, _)| (f.to_string(), *n)).collect();
    assert_eq!(
        found, ledger,
        "a vendor child or a protected launch key is built outside launch/: route it through \
         launch::spawn_agent / assign_seat, or (for a site a later slice converts) record it \
         exactly in LEDGER. Counts may only shrink."
    );
    for (_, _, why) in LEDGER {
        assert!(!why.is_empty());
    }
    let total: usize = LEDGER.iter().map(|(_, n, _)| n).sum();
    assert!(
        total <= CEILING,
        "the ledger may only shrink: {total} > {CEILING}"
    );
}

#[test]
fn scanner_catches_every_mutant_spelling() {
    let mutants = [
        "#[cfg(test)]\nmod t {\n}\nfn f() { Command::new(\"claude\"); }",
        "fn f() { let c = std::process::Command::new(\"claude\"); }",
        "use std::process::Command as Spawn;\nfn f() { Spawn::new(\"codex\"); }",
        "use tokio::process::Command as T;\nfn f() { T::new(\"agy\"); }",
        "fn f() { Command::new(resolve_agent_program(\"claude\")); }",
        "fn f() { Command::new(HeadlessVendor::Codex.program()); }",
        "fn f() { Command::new(&plan.program); }",
        "fn f(c: &mut Command) { c.env(\"AIDA_SESSION_ROLE\", \"advisor\"); }",
        "fn f(c: &mut Command) { c.env(crate::gate::seat_authority::GRANT_ENV, g); }",
        "fn f(c: &mut Command) { c.env_remove(\"AIDA_CONTAINED\"); }",
        "fn f() { std::env::set_var(\"AIDA_SESSION_ROLE\", r); }", // allow-raw-env-write: scanner fixture string, not a write
        "fn f() { std::env::remove_var(\"AIDA_CONTAINED\"); }", // allow-raw-env-write: scanner fixture string, not a write
        "fn f(c: &mut Command) { c.envs([(\"AIDA_CONTAINMENT_POSTURE\", \"none\")]); }",
        "fn f(c: &mut Command) { c.env(aida_core::launch_policy::ROLE_ENV, r); }",
    ];
    for src in mutants {
        assert!(markers(src) > 0, "scanner missed: {src}");
    }
    let clean = [
        "fn f() { Command::new(\"git\"); }",
        "// Command::new(\"claude\") in a comment\nfn f() {}",
        "fn f() {}\n#[cfg(test)]\nmod tests {\n fn t() { Command::new(\"claude\"); }\n}",
        "#[cfg(test)]\n#[path = \"x.rs\"]\nmod x;\nfn f() {}",
        "fn f(c: &mut Command) { c.env(\"AIDA_HEADLESS\", \"1\"); }",
    ];
    for src in clean {
        assert_eq!(markers(src), 0, "false positive: {src}");
    }
}
