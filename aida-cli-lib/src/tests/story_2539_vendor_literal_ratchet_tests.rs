//! Production vendor debt, keyed by source path and enclosing Rust item.
//! Counts may only shrink: move behavior behind vendors/, never expand this ledger.
//! Token scanning includes macro inputs and decoded raw/escaped strings. Typed
//! references are conservatively counted by enum/variant name (including aliases),
//! covering match arms, Self variants, imports and references outside matches.
// trace:STORY-2539 | ai:codex
use proc_macro2::{Span, TokenStream, TokenTree};
use std::{collections::BTreeMap, path::Path};
use syn::{
    spanned::Spanned,
    visit::{self, Visit},
};

type Inventory = BTreeMap<String, usize>;

#[derive(Default)]
struct Items {
    stack: Vec<String>,
    scopes: Vec<(Span, String)>,
    excluded: Vec<Span>,
}
fn test_attrs(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("test")
            || (a.path().is_ident("cfg")
                && matches!(&a.meta, syn::Meta::List(l) if l.tokens.to_string() == "test"))
    })
}
fn inside(pos: proc_macro2::LineColumn, span: Span) -> bool {
    (pos.line, pos.column) >= (span.start().line, span.start().column)
        && (pos.line, pos.column) < (span.end().line, span.end().column)
}
impl Items {
    fn enter(&mut self, span: Span, name: String) {
        self.stack.push(name);
        self.scopes.push((span, self.stack.join("::")));
    }
}
impl<'ast> Visit<'ast> for Items {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        use syn::Item::*;
        let (name, attrs) = match item {
            Fn(i) => (i.sig.ident.to_string(), &i.attrs),
            Mod(i) => (i.ident.to_string(), &i.attrs),
            Impl(i) => {
                let ty = match &*i.self_ty {
                    syn::Type::Path(p) => p
                        .path
                        .segments
                        .iter()
                        .map(|s| s.ident.to_string())
                        .collect::<Vec<_>>()
                        .join("::"),
                    _ => "anonymous".into(),
                };
                let tr = i
                    .trait_
                    .as_ref()
                    .map(|(_, p, _)| p.segments.last().unwrap().ident.to_string())
                    .unwrap_or_default();
                (format!("impl({ty}:{tr})"), &i.attrs)
            }
            Struct(i) => (i.ident.to_string(), &i.attrs),
            Enum(i) => (i.ident.to_string(), &i.attrs),
            Trait(i) => (i.ident.to_string(), &i.attrs),
            Type(i) => (i.ident.to_string(), &i.attrs),
            Const(i) => (i.ident.to_string(), &i.attrs),
            Static(i) => (i.ident.to_string(), &i.attrs),
            Union(i) => (i.ident.to_string(), &i.attrs),
            Use(i) => ("use".into(), &i.attrs),
            Macro(i) => (
                i.ident
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| i.mac.path.segments.last().unwrap().ident.to_string()),
                &i.attrs,
            ),
            ForeignMod(i) => ("extern".into(), &i.attrs),
            ExternCrate(i) => (i.ident.to_string(), &i.attrs),
            TraitAlias(i) => (i.ident.to_string(), &i.attrs),
            _ => panic!("unattributed Rust item"),
        };
        if test_attrs(attrs) {
            self.excluded.push(item.span());
            return;
        }
        self.enter(item.span(), name);
        visit::visit_item(self, item);
        self.stack.pop();
    }
    fn visit_impl_item(&mut self, i: &'ast syn::ImplItem) {
        let (name, attrs) = match i {
            syn::ImplItem::Fn(i) => (i.sig.ident.to_string(), &i.attrs),
            syn::ImplItem::Const(i) => (i.ident.to_string(), &i.attrs),
            syn::ImplItem::Type(i) => (i.ident.to_string(), &i.attrs),
            syn::ImplItem::Macro(i) => ("macro".into(), &i.attrs),
            _ => panic!("unattributed impl item"),
        };
        if test_attrs(attrs) {
            self.excluded.push(i.span());
            return;
        }
        self.enter(i.span(), name);
        visit::visit_impl_item(self, i);
        self.stack.pop();
    }
    fn visit_trait_item(&mut self, i: &'ast syn::TraitItem) {
        let (name, attrs) = match i {
            syn::TraitItem::Fn(i) => (i.sig.ident.to_string(), &i.attrs),
            syn::TraitItem::Const(i) => (i.ident.to_string(), &i.attrs),
            syn::TraitItem::Type(i) => (i.ident.to_string(), &i.attrs),
            syn::TraitItem::Macro(i) => ("macro".into(), &i.attrs),
            _ => panic!("unattributed trait item"),
        };
        if test_attrs(attrs) {
            self.excluded.push(i.span());
            return;
        }
        self.enter(i.span(), name);
        visit::visit_trait_item(self, i);
        self.stack.pop();
    }
    fn visit_attribute(&mut self, a: &'ast syn::Attribute) {
        // Rust doc comments become #[doc = "..."] tokens. Prose is out of scope.
        if a.path().is_ident("doc") {
            self.excluded.push(a.span());
        }
    }
}

fn ident_name(ident: &proc_macro2::Ident) -> String {
    let name = ident.to_string();
    name.strip_prefix("r#").unwrap_or(&name).to_string()
}

fn flatten(tokens: TokenStream, out: &mut Vec<TokenTree>) {
    for t in tokens {
        if let TokenTree::Group(g) = t {
            flatten(g.stream(), out);
        } else {
            out.push(t);
        }
    }
}
fn scan(path: &str, src: &str) -> Inventory {
    let file =
        syn::parse_file(src).unwrap_or_else(|e| panic!("{path}: cannot attribute Rust: {e}"));
    if test_attrs(&file.attrs) {
        return Inventory::new();
    }
    let mut items = Items::default();
    items.visit_file(&file);
    let mut tokens = Vec::new();
    flatten(src.parse().unwrap(), &mut tokens);
    // Precompute ordered source ranges: repeatedly resolving every excluded
    // doc/test span for every token makes large source files quadratic.
    let mut excluded: Vec<_> = items
        .excluded
        .iter()
        .map(|s| {
            let (a, b) = (s.start(), s.end());
            ((a.line, a.column), (b.line, b.column))
        })
        .collect();
    excluded.sort_unstable();
    tokens.retain(|t| {
        let p = t.span().start();
        let p = (p.line, p.column);
        let n = excluded.partition_point(|(start, _)| *start <= p);
        n == 0 || p >= excluded[n - 1].1
    });
    let mut names: BTreeMap<String, String> = [
        "LaunchVendor",
        "HeadlessVendor",
        "Claude",
        "Codex",
        "Agy",
        "Antigravity",
    ]
    .into_iter()
    .map(|s| (s.into(), s.into()))
    .collect();
    // Imports may rename both enum types and individual variants. Count uses of
    // the alias as the original token, so an already imported alias cannot hide
    // an added match arm. Iterate to cover chains of aliases.
    loop {
        let before = names.len();
        for w in tokens.windows(3) {
            if let [TokenTree::Ident(from), TokenTree::Ident(as_), TokenTree::Ident(to)] = w {
                if as_ == "as" {
                    if let Some(name) = names.get(&ident_name(from)).cloned() {
                        names.insert(ident_name(to), name);
                    }
                }
            }
        }
        if names.len() == before {
            break;
        }
    }
    let mut result = Inventory::new();
    for token in tokens {
        let pos = token.span().start();
        let kind = match token {
            TokenTree::Literal(l) => syn::parse_str::<syn::LitStr>(&l.to_string())
                .ok()
                .map(|l| l.value())
                .filter(|v| ["claude", "codex", "agy", "antigravity"].contains(&v.as_str()))
                .map(|v| format!("literal:{v}")),
            TokenTree::Ident(i) => names.get(&ident_name(&i)).map(|n| format!("typed:{n}")),
            _ => None,
        };
        if let Some(kind) = kind {
            let item = items
                .scopes
                .iter()
                .rev()
                .find(|(s, _)| inside(pos, *s))
                .map(|(_, n)| n.as_str())
                .unwrap_or("<file>");
            *result.entry(format!("{path}|{item}|{kind}")).or_default() += 1;
        }
    }
    result
}
fn inventory(workspace: &Path) -> Inventory {
    let mut result = Inventory::new();
    for (path, src) in crate::seat_clone::clone_git_ratchet_tests::workspace_sources(workspace) {
        if path
            .split('/')
            .any(|p| ["vendors", "fixtures", "docs"].contains(&p))
        {
            continue;
        }
        result.extend(scan(&path, &src));
    }
    result
}
fn ledger(text: &str) -> Inventory {
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (key, count) = l.rsplit_once('\t').expect("ledger key TAB count");
            (key.into(), count.parse().unwrap())
        })
        .collect()
}
fn violations(actual: &Inventory, allowed: &Inventory) -> Vec<String> {
    let mut errors = Vec::new();
    for (key, count) in actual {
        if allowed.get(key) != Some(count) {
            errors.push(format!("{key}: measured {count}, ledger {:?}; additions forbidden, reductions must lower ledger", allowed.get(key)));
        }
    }
    for key in allowed.keys() {
        if !actual.contains_key(key) {
            errors.push(format!("{key}: removed debt; remove ledger entry"));
        }
    }
    errors
}
#[test]
fn story_2539_vendor_literal_ratchet() {
    let actual = inventory(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap());
    let errors = violations(
        &actual,
        &ledger(include_str!("story_2539_vendor_literals.tsv")),
    );
    assert!(
        errors.is_empty(),
        "vendor debt ratchet:\n{}",
        errors.join("\n")
    );
}
#[test]
fn story_2539_vendor_literal_ratchet_mutants() {
    let base = r#"use crate::LaunchVendor as V; use V::Claude as C;
        fn existing(v: V) { match v { C => (), _ => () } }"#;
    let allowed = scan("src/example.rs", base);
    for mutant in [
        format!("{base}\nfn added() {{ let _ = \"codex\"; }}"),
        base.replace("_ => ()", "V::Codex => (), _ => ()"),
        base.replace("_ => ()", "C if true => (), _ => ()"),
        base.replace("_ => ()", "r#C if true => (), _ => ()"),
        format!("{base}\nfn alias() {{ let _ = r#\"antigravity\"#; }}"),
        base.replace("fn existing", "fn relocated"),
    ] {
        assert!(
            !violations(&scan("src/example.rs", &mutant), &allowed).is_empty(),
            "missed {mutant}"
        );
    }
    assert!(violations(&allowed, &allowed).is_empty());
    assert!(!violations(&Inventory::new(), &allowed).is_empty());
    let ignored = format!("{base}\n// \"codex\"\n#[cfg(test)] mod nested {{ fn test() {{ let _ = \"codex\"; }} }}\n#[test] fn lone() {{ let _ = \"agy\"; }}");
    assert_eq!(scan("src/example.rs", &ignored), allowed);
    assert_eq!(
        scan(
            "src/example.rs",
            &format!("{base}\n#[cfg(test)] mod t {{ use V::Codex as existing; }}")
        ),
        allowed
    );
    assert!(scan(
        "src/test_only.rs",
        "#![cfg(test)] fn only() { m!(\"codex\"); }"
    )
    .is_empty());
    assert!(scan("src/doc.rs", "#[doc = \"codex\"] fn prose() {}").is_empty());
    assert_eq!(
        scan(
            "src/x.rs",
            r##"fn f() { m!("codex", r#"claude"#, "\u{61}gy"); }"##
        )
        .values()
        .sum::<usize>(),
        3
    );
}
