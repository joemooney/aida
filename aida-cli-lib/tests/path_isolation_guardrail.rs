//! Raw PATH setters must enforce single-test process isolation.
// trace:BUG-1994 | ai:codex
use std::collections::HashSet;
use std::path::Path;
use syn::visit::{self, Visit};

#[derive(Default)]
struct Guard {
    path_names: HashSet<String>,
    setter_aliases: HashSet<String>,
    scope_checked: bool,
    offenders: usize,
}

impl Guard {
    fn is_path(&self, expr: &syn::Expr) -> bool {
        match expr {
            syn::Expr::Lit(lit) => matches!(&lit.lit, syn::Lit::Str(s) if s.value() == "PATH"),
            syn::Expr::Path(p) => p
                .path
                .get_ident()
                .is_some_and(|i| self.path_names.contains(&i.to_string())),
            syn::Expr::Reference(r) => self.is_path(&r.expr),
            syn::Expr::Paren(p) => self.is_path(&p.expr),
            _ => false,
        }
    }

    fn bind(&mut self, ident: &syn::Ident, expr: &syn::Expr) {
        if self.is_path(expr) {
            self.path_names.insert(ident.to_string());
        }
    }

    fn import(&mut self, tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(p) => self.import(&p.tree),
            syn::UseTree::Group(g) => {
                for tree in &g.items {
                    self.import(tree);
                }
            }
            syn::UseTree::Rename(r) if r.ident == "set_var" => {
                self.setter_aliases.insert(r.rename.to_string());
            }
            _ => {}
        }
    }

    fn macro_setters(&mut self, tokens: proc_macro2::TokenStream) {
        let tokens: Vec<_> = tokens.into_iter().collect();
        for pair in tokens.windows(2) {
            if let (proc_macro2::TokenTree::Ident(name), proc_macro2::TokenTree::Group(args)) =
                (&pair[0], &pair[1])
            {
                if (name == "set_var" || self.setter_aliases.contains(&name.to_string()))
                    && args.delimiter() == proc_macro2::Delimiter::Parenthesis
                {
                    let first = args.stream().into_iter().next();
                    let path = match first {
                        Some(proc_macro2::TokenTree::Literal(lit)) => {
                            syn::parse_str::<syn::LitStr>(&lit.to_string())
                                .is_ok_and(|lit| lit.value() == "PATH")
                        }
                        Some(proc_macro2::TokenTree::Ident(i)) => {
                            self.path_names.contains(&i.to_string())
                        }
                        _ => false,
                    };
                    if path && !self.scope_checked {
                        self.offenders += 1;
                    }
                }
            }
        }
        for token in tokens {
            if let proc_macro2::TokenTree::Group(group) = token {
                self.macro_setters(group.stream());
            }
        }
    }
}

impl<'ast> Visit<'ast> for Guard {
    fn visit_block(&mut self, block: &'ast syn::Block) {
        let saved_check = self.scope_checked;
        let saved_names = self.path_names.clone();
        visit::visit_block(self, block);
        self.scope_checked = saved_check;
        self.path_names = saved_names;
    }
    fn visit_expr_closure(&mut self, closure: &'ast syn::ExprClosure) {
        let saved = self.scope_checked;
        self.scope_checked = false;
        visit::visit_expr_closure(self, closure);
        self.scope_checked = saved;
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        self.import(&item.tree);
    }
    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        self.bind(&item.ident, &item.expr);
        visit::visit_item_const(self, item);
    }
    fn visit_local(&mut self, local: &'ast syn::Local) {
        if let (syn::Pat::Ident(pat), Some(init)) = (&local.pat, &local.init) {
            self.bind(&pat.ident, &init.expr);
        }
        visit::visit_local(self, local);
    }
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        let saved = self.scope_checked;
        self.scope_checked = false;
        visit::visit_item_fn(self, item);
        self.scope_checked = saved;
    }
    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        let saved = self.scope_checked;
        self.scope_checked = false;
        visit::visit_impl_item_fn(self, item);
        self.scope_checked = saved;
    }
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = call.func.as_ref() {
            if let Some(last) = path.path.segments.last() {
                if last.ident == "require_path_mutation_scope"
                    && call.args.first().is_some_and(|arg| self.is_path(arg))
                {
                    self.scope_checked = true;
                }
                let setter = last.ident == "set_var"
                    || (path.path.segments.len() == 1
                        && self.setter_aliases.contains(&last.ident.to_string()));
                if setter
                    && call.args.first().is_some_and(|arg| self.is_path(arg))
                    && !self.scope_checked
                {
                    self.offenders += 1;
                }
            }
        }
        visit::visit_expr_call(self, call);
    }
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.macro_setters(mac.tokens.clone());
    }
}

fn count(src: &str) -> usize {
    let file = syn::parse_file(src).expect("source must parse");
    let mut guard = Guard::default();
    guard.visit_file(&file);
    guard.offenders
}

fn scan(dir: &Path, offenders: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            scan(&path, offenders);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let source = std::fs::read_to_string(&path).unwrap();
            let n = count(&source);
            if n > 0 {
                offenders.push(format!("{}: {n} unguarded PATH setter(s)", path.display()));
            }
        }
    }
}

#[test]
fn raw_path_setters_require_process_isolation() {
    let mut offenders = Vec::new();
    scan(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut offenders,
    );
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}

#[test]
fn guard_rejects_direct_aliased_and_macro_path_setters() {
    for src in [
        r#"fn t() { unsafe { std::env::set_var("PATH", "fake"); } }"#,
        r#"use std::env::set_var as change; fn t() { change("PATH", "fake"); }"#,
        r#"const KEY: &str = "PATH"; fn t() { std::env::set_var(KEY, "fake"); }"#,
        r#"fn t() { wrapper!(std::env::set_var("PATH", "fake")); }"#,
        r#"fn t() { if false { crate::test_env::require_path_mutation_scope("PATH"); } std::env::set_var("PATH", "fake"); }"#,
        r#"fn t() { let unused = || { crate::test_env::require_path_mutation_scope("PATH"); }; std::env::set_var("PATH", "fake"); }"#,
        r#"fn t() { crate::test_env::require_path_mutation_scope("PATH"); } fn unguarded() { std::env::set_var("PATH", "fake"); }"#,
    ] {
        assert_eq!(count(src), 1, "{src}");
    }
    assert_eq!(
        count(
            r#"fn t() { crate::test_env::require_path_mutation_scope("PATH"); unsafe { std::env::set_var("PATH", "fake"); } }"#
        ),
        0
    );
    assert_eq!(
        count(r#"fn t() { std::process::Command::new("git").env("PATH", "fake"); }"#),
        0
    );
}
