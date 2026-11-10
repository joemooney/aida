//! STORY-2326 ratchet (TASK-1749 C2 style: a token scanner, no parser
//! dependency). Outside `launch/` and `aida_core::launch_policy`, production
//! source may not construct a vendor child or write a protected launch key,
//! except at the exact sites in [`LEDGER`]. The ledger may only shrink: each
//! later slice converts sites and lowers its counts, and it reaches zero
//! when the last slice lands. A new site, in any crate, fails here.
// trace:STORY-2326 | ai:claude

// trace:TASK-3429 | ai:codex
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const CRATES: &[&str] = &["aida-mcp", "aida-core", "aida-cli-lib", "aida-tui"];

/// Modules that are the launcher itself.
const LAUNCHER: &[&str] = &["aida-cli-lib/src/launch/", "aida-core/src/launch_policy.rs"];

/// (file, markers, why it is not converted yet). Exact counts; the total
/// may only fall (see [`CEILING`]).
/// Entries marked "not a vendor child" are tools (gh, glab, the aida binary,
/// a pane or terminal helper, process-group `pre_exec` setup) whose program
/// spelling the scanner cannot tell from a vendor's; PR-C's fn-keyed ledger
/// allowlists them by site. "Generic helper" entries take their program from
/// their callers, which PR-C classifies.
const LEDGER: &[(&str, usize, &str)] = &[
    ("aida-cli-lib/src/agent_codex_background.rs", 8, "slice C: STORY-2254 detached Codex worker (landed on main after slice A)"),
    ("aida-cli-lib/src/agent_launch.rs", 6, "slice C: Codex grant/finalize writes; slice D: the harness version probe"),
    ("aida-cli-lib/src/agent_registry.rs", 2, "not a vendor child: terminal title restore and probe"),
    ("aida-cli-lib/src/claude_agents.rs", 1, "slice D: vendor query (agents list)"),
    ("aida-cli-lib/src/codex_tool_env.rs", 2, "Codex probe and cfg(test) tool shell constructor; finalization moved into launch/profile.rs"),
    ("aida-cli-lib/src/compete_cmd.rs", 2, "slice C: remaining compete grant/finalize markers (vendor construction migrated in B)"),
    ("aida-cli-lib/src/containment/supervisor.rs", 3, "STORY-2238: the containment supervisor, a permitted constructor module"),
    ("aida-cli-lib/src/criteria_red_run.rs", 1, "generic helper: runs a criteria command"),
    ("aida-cli-lib/src/deputy/worker.rs", 3, "generic helper: deputy job argv; pre_exec process group"),
    ("aida-cli-lib/src/deputy_shell/runner.rs", 3, "not a vendor child: STORY-2338's script deputy runs bash under the operator's grant (the grant write), with a pre_exec process group (from main)"),
    ("aida-cli-lib/src/dev_build.rs", 2, "not a vendor child: the built aida binary smoke test"),
    ("aida-cli-lib/src/dev_cmd.rs", 1, "not a vendor child: resolved dev tool"),
    ("aida-cli-lib/src/scaffold_cmd.rs", 1, "slice D: vendor query (codex version)"),
    ("aida-cli-lib/src/external_cmd.rs", 1, "not a vendor child: exec of an external aida-* subcommand"),
    ("aida-cli-lib/src/forge.rs", 1, "not a vendor child: forge CLI (gh/glab)"),
    ("aida-cli-lib/src/gate/rework_hold_release.rs", 1, "not a vendor child: forge CLI"),
    ("aida-cli-lib/src/human.rs", 1, "not a vendor child: terminal send-keys"),
    ("aida-cli-lib/src/implementer_preflight.rs", 2, "not a vendor child: version probe; pre_exec process group"),
    ("aida-cli-lib/src/init_bootstrap.rs", 1, "generic helper: init runs git/forge tools"),
    ("aida-cli-lib/src/init_cmd.rs", 6, "slice D: vendor queries (MCP list/add)"),
    ("aida-cli-lib/src/kernel.rs", 2, "not a vendor child: gh binary probe"),
    ("aida-cli-lib/src/lane_env.rs", 1, "the in-process lane seat correction (Q2), not a spawn"),
    ("aida-cli-lib/src/lane_job.rs", 1, "not a vendor child: BUG-2383's sandboxed lane job runner (a cargo job, from main)"),
    ("aida-cli-lib/src/lib_part5.rs", 1, "slice D: vendor adapter command"),
    ("aida-cli-lib/src/mail_wake.rs", 1, "not a vendor child: the mail-wake helper binary"),
    ("aida-cli-lib/src/mailbox_wake_run.rs", 1, "slice C: Codex thread submit"),
    ("aida-cli-lib/src/mandate_drive.rs", 1, "generic helper: scrubbed_command"),
    ("aida-cli-lib/src/manual.rs", 1, "not a vendor child: pager"),
    ("aida-mcp/src/mcp.rs", 1, "not a vendor child: exec of the aida MCP server"),
    ("aida-cli-lib/src/merge_hold.rs", 3, "not a vendor child: forge CLI"),
    ("aida-cli-lib/src/pane_host.rs", 1, "not a vendor child: pane host argv"),
    ("aida-cli-lib/src/phase_driver.rs", 4, "slice C: phase advisor and auto-punt control child"),
    ("aida-cli-lib/src/phase_driver_trait.rs", 3, "slice C: CI-fix launch"),
    ("aida-cli-lib/src/pr_cmd.rs", 1, "not a vendor child: pre_exec process group"),
    ("aida-cli-lib/src/regen/score.rs", 2, "not a vendor child: scoring sandbox (argv, pre_exec)"),
    ("aida-cli-lib/src/relay_mail.rs", 1, "not a vendor child: pre_exec process group"),
    ("aida-cli-lib/src/remote_create.rs", 1, "not a vendor child: forge CLI"),
    ("aida-cli-lib/src/review_watch.rs", 1, "not a vendor child: the watch command"),
    ("aida-cli-lib/src/review_watch_forge.rs", 4, "not a vendor child: forge CLI"),
    ("aida-cli-lib/src/seat_target_overlay.rs", 1, "not a vendor child: the overlay probe runs the lane-job bwrap on `true` under unshare (from main)"),
    ("aida-cli-lib/src/session.rs", 41, "slice A2: the three Claude resume paths; slice C: remaining Codex finalization/exec and vendor resume markers; the shared seat-env helper; the contained export (its aida control children convert in slice C); the interactive exec of a built launch"),
    ("aida-cli-lib/src/session_start.rs", 2, "not a vendor child: forge CLI"),
    ("aida-cli-lib/src/shift.rs", 2, "generic helper: shift command; pre_exec process group"),
    ("aida-cli-lib/src/test_exec.rs", 1, "test-support helper compiled into the crate"),
    ("aida-cli-lib/src/transport_scan.rs", 1, "not a vendor child: pre_exec process group"),
    ("aida-cli-lib/src/upgrade_cmd.rs", 1, "not a vendor child: upgrade installer"),
    ("aida-core/src/ai/client.rs", 1, "slice D: the AI client's vendor CLI request"),
    ("aida-core/src/db/refresh_worker.rs", 1, "not a vendor child: pre_exec process group"),
    ("aida-core/src/proc_identity.rs", 1, "not a vendor launch: shared cooperative pre_exec self-check receipt; does not construct or spawn a command"),
    ("aida-tui/src/actions.rs", 1, "generic helper: TUI action argv"),
    ("aida-tui/src/dispatch.rs", 1, "generic helper: TUI run_child"),
];

/// The slice-A total. A later slice lowers it; it is zero after slice D.
/// (Advisor ruling 1/2 widened the scanner to every dynamic program and
/// `exec`/`pre_exec`; the sites were already there.) The merge of main
/// brought in STORY-2254's detached Codex worker (+8, ledgered above). The
/// merges after 0ff32132 brought in +3 (lane_job.rs, seat_target_overlay.rs,
/// and codex_tool_env.rs's cfg(test) fn), all ledgered above. The rebase onto
/// main 339203a5 brought in +3 (STORY-2338's deputy_shell/runner.rs), ledgered.
// PR-B removes 13 markers: compete (-3), dispatch (-3), phase builders
// (their legacy Aida markers remain), fresh Codex session/exec (-7).
// trace:STORY-2326 | ai:codex
// PR-C moves finalization (-1), phase control (-1), gate control (-1),
// zen vendor/seat (-3), and TUI control seats (-12) into the launcher.
// Coordinator seat writes (-4), queue process role export (-1), exec owner (-1).
// Shared cooperative identity capture adds one pre_exec self-check receipt,
// not a vendor command/spawn; the exact file/count/reason is ledgered above.
// trace:STORY-2537 trace:BUG-2458 | ai:codex
const CEILING: usize = 134;

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

/// The exact spellings of a non-vendor program (the aida binary, git, gh,
/// glab, a hook, script or shell) the production sources pass to
/// `Command::new`. A non-literal program is a launch marker unless its
/// spelling (a value, or a call's callee path, see [`program_spelling`]) is
/// listed here verbatim; a word inside it never exempts it, so
/// `claude_exe`, `vendor_tmp` or `codex_bin` count (advisor ruling 3, round
/// 5). A new non-vendor spelling is added here deliberately.
// trace:STORY-2326 | ai:claude
const NON_VENDOR_SPELLINGS: &[&str] = &[
    // Values.
    "aida",
    "aida_bin",
    "bwrap",
    "dev_bin",
    "exe",
    "fixture.gh",
    "gh",
    "gh_bin",
    "glab",
    "hook",
    "plan.exe",
    "real_git",
    "sccache_bin",
    "script",
    "server_bin",
    "shell",
    // Callee paths.
    "aida_exe_path",
    "crate::aida_bin::current_executable_path",
    "crate::aida_exe_path",
    // The aida binary (BUG-2436's finished-lane `aida session end`, from main).
    "crate::kernel::aida_exe_path",
    "crate::kernel::resolve_gh_binary_for_probe",
    "crate::pr_cmd::pr_ship_post_merge_aida_exe",
    "crate::resolve_aida_exe",
    "glab_binary",
    "pr_ship_post_merge_aida_exe",
    "resolve_aida_exe",
    "self.aida_exe",
    "std::env::current_exe",
];

/// The spelling of a `Command::new` argument: the argument itself, or for a
/// call (`crate::aida_exe_path()`, `resolve_aida_exe().join(..)`) its callee path,
/// the text before the first `(`. A trailing `,` or `?` is dropped.
fn program_spelling(arg: &str) -> &str {
    let arg = arg.trim_start_matches('&').trim_end_matches([',', '?']);
    arg.split_once('(').map_or(arg, |(callee, _)| callee)
}

/// Whether a `Command::new` argument names the program of a vendor child.
/// A string literal is one only when it is a vendor name; anything else is
/// one unless its spelling is in [`NON_VENDOR_SPELLINGS`]. Vendor resolvers
/// always count, so `let program = resolve_agent_program(..);
/// Command::new(program)` and a program passed in as a parameter are both
/// seen.
fn vendor_program_arg(arg: &str) -> bool {
    let arg = arg.trim_start_matches('&');
    if arg.starts_with('"') {
        return VENDORS.iter().any(|v| arg == format!("\"{v}\""));
    }
    if [
        "resolve_agent_program",
        ".program()",
        "plan.program",
        "resolve_vendor_bin",
    ]
    .iter()
    .any(|m| arg.contains(m))
    {
        return true;
    }
    !NON_VENDOR_SPELLINGS.contains(&program_spelling(arg))
}

/// Calls of `name(` in `normalized` that start an identifier (so
/// `Command::new(` does not match inside `GitCommand::new(`).
fn calls<'a>(normalized: &'a str, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    normalized.match_indices(name).filter_map(move |(at, _)| {
        let before = normalized[..at].chars().next_back();
        let starts = !before.is_some_and(|c| c.is_alphanumeric() || c == '_');
        starts.then(|| &normalized[at + name.len()..])
    })
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
        let new = format!("{ctor}::new(");
        n += calls(&normalized, &new)
            .filter(|call| vendor_program_arg(call_args(call)))
            .count();
    }
    // A process that becomes another program, or code run in the child
    // before it does (`CommandExt::exec` / `pre_exec`, method or path form).
    n += [
        ".exec()",
        "CommandExt::exec(",
        ".pre_exec(",
        "CommandExt::pre_exec(",
    ]
    .iter()
    .map(|form| normalized.matches(form).count())
    .sum::<usize>();
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
        // The two-line variable form and a program passed in as a parameter.
        "fn f() { let program = resolve_agent_program(\"claude\");\n let c = Command::new(program); }",
        "fn f() { let (program, args) = os_wrapped_program_and_args(r, p, a)?; Command::new(&program); }",
        "fn f(agent_bin: &Path) { Command::new(agent_bin); }",
        "fn f(launch: L) { tokio::process::Command::new(launch.program); }",
        "fn f() { let c = Command::new(vendor_bin()); }",
        // Ruling 3 (round 5): a non-vendor word inside a name never exempts it.
        "fn f() { let claude_exe = resolve_vendor(\"claude\");\n Command::new(&claude_exe); }",
        "fn f(vendor_tmp: &Path) { Command::new(&vendor_tmp); }",
        "fn f() { Command::new(fake_shell_codex); Command::new(dev_server_agent()); }",
        "fn f() { Command::new(exe_of(\"claude\")); Command::new(&aida_exe.join(\"x\")); }",
        // A process becoming the vendor, and child-side code before it does.
        "fn f(mut c: Command) { use std::os::unix::process::CommandExt; c.exec(); }",
        "fn f(mut c: Command) { std::os::unix::process::CommandExt::exec(&mut c); }",
        "fn f(c: &mut Command) { unsafe { c.pre_exec(|| Ok(())); } }",
        "fn f(c: &mut Command) { unsafe { CommandExt::pre_exec(c, || Ok(())); } }",
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
        "fn f() { Command::new(crate::aida_exe_path()); Command::new(&gh_bin); }",
        // Split so TASK-1262's raw-lookup ratchet does not count the fixture.
        concat!(
            "fn f() { Command::new(&real_git); Command::new(std::env::current_",
            "exe()?); }"
        ),
        "fn f() { GitCommand::new(program); }",
    ];
    for src in clean {
        assert_eq!(markers(src), 0, "false positive: {src}");
    }
}

// Match identifiers as well as qualified calls so imports (including grouped
// imports and aliases) cannot hide the backend behind a bare call.
// trace:STORY-2326 | ai:codex
fn has_legacy_child_issuer(src: &str) -> bool {
    let backend = src.replace("crate::launch::issue_launch_child_with_delegation", "");
    backend
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|word| {
            matches!(
                word,
                "issue_launch_child_with_delegation" | "issue_child_from" | "issue_child"
            )
        })
}

#[test]
fn story_2326_grant_owner_mutants() {
    for mutant in [
        "seat_authority::issue_launch_child_with_delegation(root)",
        "use crate::gate::seat_authority::issue_launch_child_with_delegation; issue_launch_child_with_delegation(root)",
        "use crate::gate::seat_authority::issue_launch_child_with_delegation as mint; mint(root)",
        "use crate::gate::seat_authority::{issue_launch_child_with_delegation as mint}; mint(root)",
        "use crate::gate::seat_authority::issue_child_from as mint; mint(root)",
        "seat_authority::issue_child(root)",
        "use crate::gate::seat_authority::issue_child; issue_child(root)",
        "use crate::gate::seat_authority::issue_child as mint; mint(root)",
        "use crate::gate::seat_authority::{issue_child as mint}; mint(root)",
    ] {
        assert!(has_legacy_child_issuer(mutant), "escaped: {mutant}");
    }
    for allowed in [
        "crate::launch::issue_launch_child(root)",
        "crate::launch::issue_launch_child_with_delegation(root)",
    ] {
        assert!(!has_legacy_child_issuer(allowed), "valid facade: {allowed}");
    }
}

/// Finalization and grant issuance remain owned by the launch layer even for
/// the inventoried builders awaiting conversion.
// trace:STORY-2326 | ai:codex
#[test]
fn story_2326_profile_and_grant_issuance_have_one_owner() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    for file in files {
        let rel = file.strip_prefix(&root).unwrap().to_string_lossy();
        if rel.ends_with("_tests.rs") || rel.ends_with("tests.rs") {
            continue;
        }
        let src = production(&std::fs::read_to_string(&file).unwrap());
        if rel != "launch/profile.rs" {
            assert!(
                !src.contains("fn prepare_launch("),
                "private profile finalizer escaped into {rel}"
            );
        }
        if !rel.starts_with("launch/") && rel != "gate/seat_authority.rs" {
            assert!(
                !src.contains("issue_child_for("),
                "direct grant backend caller in {rel}"
            );
            assert!(
                !has_legacy_child_issuer(&src),
                "legacy child issuer in {rel}"
            );
        }
        assert!(
            !src.contains("seat_authority::issue_launch_child("),
            "legacy launch issuer in {rel}"
        );
    }
    let profile = std::fs::read_to_string(root.join("launch/profile.rs")).unwrap();
    assert!(profile.contains("fn prepare_launch("));
    assert!(!profile.contains("pub(crate) fn prepare_launch("));
}

/// The pure selector policy can only be applied by the two checked AIDA
/// constructors. Vendor code cannot reuse it to request inherited authority.
// trace:STORY-2326 | ai:codex
#[test]
fn story_2326_control_env_users_are_checked_aida_constructors() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut found = BTreeMap::new();
    for krate in CRATES {
        let mut files = Vec::new();
        rust_files(&root.join(krate).join("src"), &mut files);
        for path in files {
            if path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("_tests.rs")
            {
                continue;
            }
            let src = production(&std::fs::read_to_string(&path).unwrap());
            let normalized: String = src.split_whitespace().collect();
            let n = calls(&normalized, "control_env(").count();
            if n > 0 {
                found.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    n,
                );
            }
        }
    }
    assert_eq!(
        found,
        BTreeMap::from([
            ("aida-cli-lib/src/launch/control.rs".to_string(), 1),
            ("aida-tui/src/redesign/mod.rs".to_string(), 1),
        ]),
        "control_env must only run inside checked AIDA control constructors"
    );
}

/// Inventory every literal production AIDA name, conservatively including
/// writes as well as reads. New inputs require an explicit disposition.
// trace:STORY-2326 | ai:codex
#[test]
fn story_2326_ambient_aida_inputs_have_a_disposition() {
    use aida_core::launch_policy::{AMBIENT_AIDA_KEYS, DROPPED_AIDA_KEYS, OWNED_KEYS};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let names = regex::Regex::new(r#""(AIDA_[A-Z][A-Z0-9_]+)""#).unwrap();
    for (path, source) in crate::seat_clone::clone_git_ratchet_tests::workspace_sources(root) {
        let src = production(&source);
        for captures in names.captures_iter(&src) {
            let key = &captures[1];
            assert!(
                OWNED_KEYS.iter().any(|(name, _)| *name == key)
                    || AMBIENT_AIDA_KEYS.contains(&key)
                    || DROPPED_AIDA_KEYS.contains(&key),
                "unclassified production input {key} in {path}"
            );
        }
    }
    for key in AMBIENT_AIDA_KEYS {
        assert!(
            !key.starts_with("AIDA_TEST_")
                && !key.starts_with("AIDA_REQUIRE_")
                && !key.contains("TOKEN")
                && !key.contains("API_KEY")
                && *key != "AIDA_AGENT_CMD"
        );
        assert!(!DROPPED_AIDA_KEYS.contains(key));
    }
}
