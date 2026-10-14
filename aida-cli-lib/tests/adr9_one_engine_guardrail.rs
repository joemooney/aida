//! ADR-9: CI guardrail for the ADR-7 "one per-spec engine" invariant.
//!
//! Every per-spec driver runs the implement -> CI -> review -> merge -> pull
//! lifecycle through the ONE engine, `auto_complete::orchestrate_with_resume`.
//! `aida zen` and `aida integrate` reach it by self-invoking
//! `aida queue work --auto-complete` (a subprocess that re-enters the engine);
//! the only in-process production call site is the `queue work` handler in
//! `lib.rs`. `aida burndown` is the SINGLE sanctioned FLEET-layer exception —
//! it fans out native subagents one level above the per-spec lifecycle and
//! deliberately does NOT route through the engine.
//!
//! This test converts that invariant from prose (which failed at zen slice-1,
//! PR #1231: implement+PR with zero review) into a CI gate. See
//! docs/architecture/one-engine-invariant.md.
// trace:ADR-9 | ai:claude

use std::fs;
use std::path::{Path, PathBuf};
use syn::visit::{self, Visit};

/// Source files (relative to the crate root) allowed to call the engine
/// `orchestrate_with_resume` in PRODUCTION code. Adding a new per-spec driver?
/// Route it through the engine — either self-invoke `queue work --auto-complete`
/// (like zen/integrate, which adds NO new call site here) or, if it is a genuine
/// new in-process engine entry point, add its file here CONSCIOUSLY. Do NOT
/// reimplement the per-spec lifecycle: that is the burndown fleet-layer exception,
/// and it is the only one (see the architecture doc).
// trace:ADR-9 | ai:claude
const ENGINE_CALLER_ALLOWLIST: &[&str] = &[
    "src/auto_complete.rs", // the engine module itself (the orchestrate wrapper)
    // the `aida queue work --auto-complete` handler (STORY-772: was main.rs;
    // the STORY-1488 split moved run_auto_complete out of lib.rs)
    "src/drain_gate.rs",
];

/// The engine entry point every per-spec driver must route through.
const ENGINE_FN: &str = "orchestrate_with_resume(";

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}

/// Count production (non-comment) call lines of the engine entry point, skipping
/// the `fn orchestrate_with_resume` definition itself.
fn production_engine_calls(src: &str) -> usize {
    src.lines()
        .filter(|line| {
            let t = line.trim_start();
            line.contains(ENGINE_FN)
                && !t.starts_with("//")
                && !t.starts_with('*')
                && !line.contains("fn orchestrate_with_resume")
        })
        .count()
}

// trace:TASK-1724 | ai:codex
fn requires_test(meta: &syn::Meta) -> bool {
    match meta {
        syn::Meta::Path(path) => path.is_ident("test"),
        syn::Meta::List(list) if list.path.is_ident("all") => list
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            )
            .expect("valid cfg arguments")
            .iter()
            .any(requires_test),
        _ => false,
    }
}

// trace:TASK-1724 | ai:codex
fn macro_engine_references(tokens: proc_macro2::TokenStream) -> usize {
    tokens
        .into_iter()
        .map(|token| match token {
            proc_macro2::TokenTree::Ident(ident) if ident == "orchestrate_with_resume" => 1,
            proc_macro2::TokenTree::Group(group) => macro_engine_references(group.stream()),
            _ => 0,
        })
        .sum()
}

/// This former test-only whole-file exception must have zero production calls.
/// Parse module boundaries rather than hiding everything after the first test
/// attribute; production items can follow a test module. Comments and string
/// literals containing braces or the engine's name cannot affect the count.
// trace:TASK-1724 | ai:codex
fn upgrade_release_production_calls(src: &str) -> usize {
    struct Calls(usize);
    impl<'ast> Visit<'ast> for Calls {
        fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
            if module.attrs.iter().any(|attr| {
                attr.path().is_ident("cfg")
                    && requires_test(&attr.parse_args::<syn::Meta>().expect("valid cfg"))
            }) {
                return;
            }
            visit::visit_item_mod(self, module);
        }

        // trace:TASK-1355 | ai:claude
        // Any path naming the engine counts: a direct call's callee, but also
        // a function-pointer reference (`let go = orchestrate_with_resume;`).
        fn visit_expr_path(&mut self, path: &'ast syn::ExprPath) {
            if path
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "orchestrate_with_resume")
            {
                self.0 += 1;
            }
            visit::visit_expr_path(self, path);
        }

        // trace:TASK-1355 | ai:claude
        // The method-call form `x.orchestrate_with_resume(..)`.
        fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
            if call.method == "orchestrate_with_resume" {
                self.0 += 1;
            }
            visit::visit_expr_method_call(self, call);
        }

        // trace:TASK-1355 | ai:claude
        // An aliased import (`use ..::orchestrate_with_resume as go;`) hides
        // every later `go(..)` call from the path check, so the alias itself
        // counts.
        fn visit_use_rename(&mut self, rename: &'ast syn::UseRename) {
            if rename.ident == "orchestrate_with_resume" {
                self.0 += 1;
            }
            visit::visit_use_rename(self, rename);
        }

        fn visit_macro(&mut self, mac: &'ast syn::Macro) {
            // Macro bodies are opaque to the AST visitor. Conservatively count
            // engine references there too, rather than permitting a new bypass.
            self.0 += macro_engine_references(mac.tokens.clone());
        }
    }
    let parsed = syn::parse_file(src).expect("guarded source must parse as Rust");
    let mut calls = Calls(0);
    calls.visit_file(&parsed);
    calls.0
}

// trace:TASK-1724 | ai:codex
fn offending_calls(rel: &str, src: &str) -> usize {
    if ENGINE_CALLER_ALLOWLIST.contains(&rel) {
        0
    } else if rel == "src/upgrade_release.rs" {
        upgrade_release_production_calls(src)
    } else {
        production_engine_calls(src)
    }
}

#[test]
fn only_allowlisted_files_call_the_one_engine_in_production() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rs_files(&root.join("src"), &mut files);
    assert!(!files.is_empty(), "found no src/*.rs — test harness broken");

    let mut offenders = Vec::new();
    for f in &files {
        let rel = f
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let rel = if rel.starts_with("src/lib_part") {
            "src/lib.rs".to_string()
        } else {
            rel
        };
        let src = fs::read_to_string(f).unwrap();
        let n = offending_calls(&rel, &src);
        if n > 0 {
            offenders.push(format!("{rel} ({n} call site(s))"));
        }
    }

    assert!(
        offenders.is_empty(),
        "ADR-9 one-engine invariant violated: these files call \
         `orchestrate_with_resume` outside the allow-list. A per-spec driver must \
         route the lifecycle through the ONE engine — self-invoke \
         `aida queue work --auto-complete` (like zen/integrate), or, if this is a \
         genuine new in-process engine entry, add the file to \
         ENGINE_CALLER_ALLOWLIST with a comment. Do NOT reimplement the per-spec \
         lifecycle (burndown is the only sanctioned fleet-layer exception; see \
         docs/architecture/one-engine-invariant.md). Offenders: {offenders:?}"
    );
}

// trace:TASK-1724 | ai:codex
#[test]
fn upgrade_release_test_calls_are_excluded_from_production_count() {
    for cfg in [
        "test",
        "all(test, unix)",
        "all(unix, all(test, feature = \"x\"))",
    ] {
        let src = format!(
            r##"
            #[cfg({cfg})]
            mod tests {{
                fn harness() {{
                    let text = r#"}} orchestrate_with_resume("#;
                    // }} orchestrate_with_resume(
                    if true {{ auto_complete::orchestrate_with_resume(); }}
                }}
            }}
            "##
        );
        assert_eq!(offending_calls("src/upgrade_release.rs", &src), 0, "{cfg}");
    }
}

// trace:TASK-1724 | ai:codex
#[test]
fn allowlisted_test_only_file_production_call_is_an_offender() {
    let src = r#"
        fn before() { auto_complete::orchestrate_with_resume(); }
        #[cfg(test)]
        mod tests { fn harness() { auto_complete::orchestrate_with_resume(); } }
        mod production { fn after() { auto_complete::orchestrate_with_resume(); } }
    "#;
    assert_eq!(offending_calls("src/upgrade_release.rs", src), 2);
    // `any` does not require test mode: this module can compile in production.
    let src = r#"
        #[cfg(any(test, unix))]
        mod production { fn run() { auto_complete::orchestrate_with_resume(); } }
    "#;
    assert_eq!(offending_calls("src/upgrade_release.rs", src), 1);
    let src = "fn run() { wrapper!(auto_complete::orchestrate_with_resume()); }";
    assert_eq!(offending_calls("src/upgrade_release.rs", src), 1);
    let src = r#"fn run() { println!("orchestrate_with_resume("); }"#;
    assert_eq!(offending_calls("src/upgrade_release.rs", src), 0);
}

/// The AST scan must not be dodged by a method-call form, an aliased import,
/// or a function-pointer reference in production code.
// trace:TASK-1355 | ai:claude
#[test]
fn upgrade_release_method_alias_and_pointer_forms_are_offenders() {
    let src = "fn run(x: X) { x.orchestrate_with_resume(); }";
    assert_eq!(offending_calls("src/upgrade_release.rs", src), 1);
    let src = r#"
        use crate::auto_complete::orchestrate_with_resume as go;
        fn run() { go(); }
    "#;
    assert_eq!(offending_calls("src/upgrade_release.rs", src), 1);
    let src = "fn run() { let go = auto_complete::orchestrate_with_resume; go(); }";
    assert_eq!(offending_calls("src/upgrade_release.rs", src), 1);
    // The same forms inside a test-only module still do not count.
    let src = r#"
        #[cfg(test)]
        mod tests {
            use crate::auto_complete::orchestrate_with_resume as go;
            fn harness(x: X) { x.orchestrate_with_resume(); go(); }
        }
    "#;
    assert_eq!(offending_calls("src/upgrade_release.rs", src), 0);
}

/// The positive side of the invariant: the allow-listed `drain_gate.rs` handler must
/// still actually route through the engine, so the allow-list can't rot into a
/// dead entry that quietly permits a bypass.
#[test]
fn the_queue_work_handler_still_routes_through_the_engine() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let handler = fs::read_to_string(root.join("src/drain_gate.rs")).unwrap();
    assert!(
        production_engine_calls(&handler) >= 1,
        "ADR-9: `drain_gate.rs` no longer calls `orchestrate_with_resume` — the \
         `queue work --auto-complete` handler must route through the one engine."
    );
}
