//! TASK-2707 (STORY-2238 decision 3, threat T7): the shared SQLite cache is a
//! rebuildable read projection that contained lanes may write, so it must never
//! decide authority. Grants, roles, seats, review records, merge holds and
//! mandates are read from the store and the grant records only.
//!
//! A source ratchet: no production line in an authority module names the
//! cache. A new need for cached data in these modules goes through the store
//! (`GitBackend`, `load_store_for_lookup`), never the cache.
// trace:TASK-2707 | ai:claude

use std::path::{Path, PathBuf};

/// Spellings that reach the cache: its types, path helpers, the cached
/// backend, the generic opener that wraps every git store in the cache, and
/// raw SQLite.
const CACHE_TOKENS: &[&str] = &[
    "CachedGitBackend",
    "Cache::open",
    "aida_core::Cache",
    "db::Cache",
    "project_cache_path",
    "default_cache_path",
    "ensure_project_cache_dir",
    "cache_dir_in",
    "CACHE_DIR_NAME",
    "CACHE_FILE_NAME",
    "cache.db",
    "cache-db",
    ".cache()",
    "aida_core::open_or_create",
    "db::open_or_create",
    "aida_core::create_backend",
    "db::create_backend",
    "rusqlite",
    "requirements_cache",
];

/// Modules that decide authority: everything under `gate/` (dispatch, seat,
/// role, review and approval authority, review records and verdicts, merge
/// holds, mandates, the ledger), plus the grant keyring and the role model.
fn authority_sources(src: &Path) -> Vec<PathBuf> {
    let mut out = vec![
        src.join("grant_keyring.rs"),
        src.join("role.rs"),
        src.join("merge_hold.rs"),
        src.join("mandate.rs"),
    ];
    let mut stack = vec![src.join("gate")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                if name != "tests" {
                    stack.push(path);
                }
            } else if name.ends_with(".rs") && !name.ends_with("tests.rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// 1-based inclusive line ranges of every `#[cfg(test)]`-gated node in `src`
/// (the attribute plus the item, impl/trait member, statement, match arm,
/// field or variant it gates). A gated node of a kind not listed here stays
/// in the scan, so the ratchet errs toward scanning too much, never too
/// little. `cfg(all(.., test, ..))` counts as test-only; `cfg(any(..))` and
/// `cfg(not(test))` do not.
fn test_gated_line_ranges(src: &str) -> Result<Vec<(usize, usize)>, syn::Error> {
    use syn::spanned::Spanned;
    use syn::visit::Visit;

    fn meta_is_test(meta: &syn::Meta) -> bool {
        match meta {
            syn::Meta::Path(path) => path.is_ident("test"),
            syn::Meta::List(list) if list.path.is_ident("all") => list
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                )
                .is_ok_and(|inner| inner.iter().any(meta_is_test)),
            _ => false,
        }
    }
    fn gated(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|attr| {
            attr.path().is_ident("cfg")
                && attr
                    .parse_args::<syn::Meta>()
                    .is_ok_and(|m| meta_is_test(&m))
        })
    }

    struct Gated(Vec<(usize, usize)>);
    impl Gated {
        fn record(&mut self, node: &impl Spanned) {
            let span = node.span();
            self.0.push((span.start().line, span.end().line));
        }
    }
    macro_rules! prune {
        ($($method:ident: $ty:ty => $walk:path;)*) => {$(
            fn $method(&mut self, node: &'ast $ty) {
                if gated(&node.attrs) {
                    self.record(node);
                } else {
                    $walk(self, node);
                }
            }
        )*};
    }
    impl<'ast> Visit<'ast> for Gated {
        fn visit_item(&mut self, node: &'ast syn::Item) {
            let attrs: &[syn::Attribute] = match node {
                syn::Item::Const(i) => &i.attrs,
                syn::Item::Enum(i) => &i.attrs,
                syn::Item::ExternCrate(i) => &i.attrs,
                syn::Item::Fn(i) => &i.attrs,
                syn::Item::ForeignMod(i) => &i.attrs,
                syn::Item::Impl(i) => &i.attrs,
                syn::Item::Macro(i) => &i.attrs,
                syn::Item::Mod(i) => &i.attrs,
                syn::Item::Static(i) => &i.attrs,
                syn::Item::Struct(i) => &i.attrs,
                syn::Item::Trait(i) => &i.attrs,
                syn::Item::TraitAlias(i) => &i.attrs,
                syn::Item::Type(i) => &i.attrs,
                syn::Item::Union(i) => &i.attrs,
                syn::Item::Use(i) => &i.attrs,
                _ => &[],
            };
            if gated(attrs) {
                self.record(node);
            } else {
                syn::visit::visit_item(self, node);
            }
        }
        fn visit_impl_item(&mut self, node: &'ast syn::ImplItem) {
            let attrs: &[syn::Attribute] = match node {
                syn::ImplItem::Const(i) => &i.attrs,
                syn::ImplItem::Fn(i) => &i.attrs,
                syn::ImplItem::Type(i) => &i.attrs,
                syn::ImplItem::Macro(i) => &i.attrs,
                _ => &[],
            };
            if gated(attrs) {
                self.record(node);
            } else {
                syn::visit::visit_impl_item(self, node);
            }
        }
        fn visit_trait_item(&mut self, node: &'ast syn::TraitItem) {
            let attrs: &[syn::Attribute] = match node {
                syn::TraitItem::Const(i) => &i.attrs,
                syn::TraitItem::Fn(i) => &i.attrs,
                syn::TraitItem::Type(i) => &i.attrs,
                syn::TraitItem::Macro(i) => &i.attrs,
                _ => &[],
            };
            if gated(attrs) {
                self.record(node);
            } else {
                syn::visit::visit_trait_item(self, node);
            }
        }
        prune! {
            visit_local: syn::Local => syn::visit::visit_local;
            visit_stmt_macro: syn::StmtMacro => syn::visit::visit_stmt_macro;
            visit_arm: syn::Arm => syn::visit::visit_arm;
            visit_field: syn::Field => syn::visit::visit_field;
            visit_variant: syn::Variant => syn::visit::visit_variant;
            visit_field_value: syn::FieldValue => syn::visit::visit_field_value;
        }
    }

    let file = syn::parse_file(src)?;
    let mut gated_ranges = Gated(Vec::new());
    gated_ranges.visit_file(&file);
    Ok(gated_ranges.0)
}

/// The production part of a source file: every line outside a
/// `#[cfg(test)]`-gated node, minus `//` comments (prose may name the cache).
fn production_lines(src: &str) -> Vec<(usize, &str)> {
    let gated = test_gated_line_ranges(src).unwrap_or_else(|e| {
        panic!(
            "authority source does not parse at line {}: {e}",
            e.span().start().line
        )
    });
    src.lines()
        .enumerate()
        .map(|(n, line)| (n + 1, line))
        .filter(|(n, _)| !gated.iter().any(|(lo, hi)| (lo..=hi).contains(&n)))
        .map(|(n, line)| (n, line.split("//").next().unwrap_or("")))
        .collect()
}

fn cache_offenders(src: &str) -> Vec<(usize, &'static str)> {
    production_lines(src)
        .into_iter()
        .flat_map(|(n, code)| {
            CACHE_TOKENS
                .iter()
                .filter(move |token| code.contains(**token))
                .map(move |token| (n, *token))
        })
        .collect()
}

/// Round-1 review: the scan used to stop at the first `#[cfg(test)]`, so a
/// cache read placed after an early test-only hook went unseen. Only the gated
/// node is skipped now; everything after it is still production code.
#[test]
fn cache_read_after_an_early_test_hook_is_caught() {
    let planted = "pub struct Role {\n\
                   \x20   #[cfg(test)]\n\
                   \x20   pub probe: CachedGitBackend,\n\
                   }\n\
                   #[cfg(test)]\n\
                   fn hook() { let _ = rusqlite::Connection::open_in_memory(); }\n\
                   impl Role {\n\
                   \x20   #[cfg(test)]\n\
                   \x20   fn seam(&self) { let _ = aida_core::Cache::open(p); }\n\
                   \x20   fn authority(&self, root: &Path) -> bool {\n\
                   \x20       #[cfg(test)]\n\
                   \x20       let _ = aida_core::db::project_cache_path(root);\n\
                   \x20       let c = aida_core::db::open_or_create(root);\n\
                   \x20       c.is_ok()\n\
                   \x20   }\n\
                   }\n\
                   #[cfg(all(test, unix))]\n\
                   mod t { fn x() { CachedGitBackend::default_cache_path(p); } }\n\
                   #[cfg(not(test))]\n\
                   fn live() { let _ = rusqlite::Connection::open_in_memory(); }\n";
    let found = cache_offenders(planted);
    assert_eq!(
        found,
        vec![(13, "db::open_or_create"), (20, "rusqlite")],
        "only the ungated reads after the early test hooks are offenders"
    );
}

#[test]
fn cache_rows_never_grant_authority() {
    // Positive control: the scanner sees a planted cache read and ignores
    // comments and test code.
    let planted = "fn grant_ok(root: &Path) -> bool {\n    \
                   let c = aida_core::Cache::open(aida_core::db::project_cache_path(root));\n    \
                   c.is_ok()\n}\n\
                   // the cache.db is never authority\n\
                   #[cfg(test)]\nmod t { fn x() { CachedGitBackend::default_cache_path(p); } }\n";
    let found = cache_offenders(planted);
    assert_eq!(
        found.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        vec![2, 2, 2],
        "{found:?}"
    );

    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    // trace:TASK-3429 | ai:codex — retain coverage of the extracted transport.
    let mut sources = authority_sources(&src);
    let mcp = src.join("../../aida-mcp/src");
    for entry in std::fs::read_dir(&mcp).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "rs") {
            sources.push(path);
        }
    }
    assert!(
        sources.len() > 30,
        "the authority module set shrank unexpectedly: {sources:?}"
    );
    let mut offenders = Vec::new();
    for path in &sources {
        let text =
            std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for (line, token) in cache_offenders(&text) {
            offenders.push(format!(
                "{}:{line} names `{token}`",
                path.strip_prefix(&src).unwrap_or(path).display()
            ));
        }
    }
    assert!(
        offenders.is_empty(),
        "authority code must read the store and grant records, never the shared \
         cache (STORY-2238 T7):\n{}",
        offenders.join("\n")
    );
}
