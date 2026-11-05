//! Which lines of a Rust file a `trace:<SPEC-ID>` marker claims: the single
//! region rule shared by `aida lint constants` and whole-tree
//! `aida trace coverage`, so "traced" means one thing in both.
//!
//! - File scope: a module-level `//!` anchor claims every line of the file
//!   (the diff mode's FileAnchor rule).
//! - Block scope: a marker in an `impl`, `trait` or `enum` leading prefix or
//!   header (up to its `{` line) claims the whole block: an impl's methods, a
//!   trait's default methods, an enum's discriminants. A marker inside one
//!   method never claims its siblings.
//! - Item scope: a marker inside a function, method, trait default method,
//!   const or static, or in its leading comment/blank prefix (attached with
//!   the mandate-evidence walk), claims that item.
//! - Test code: `#[cfg(test)]` modules/items and `#[test]` fns are recorded
//!   as test spans, which both consumers exempt.
//! - Granularity is the line: a marker claims every item on its line.
// trace:TASK-1783 | ai:claude

use std::collections::BTreeSet;
use std::ops::Range;

use syn::{spanned::Spanned, visit::Visit};

/// What kind of item a region is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemKind {
    Fn,
    Impl,
    /// A trait: a marker on it claims its default methods and consts.
    // trace:TASK-1954 | ai:claude — TASK-1812 (7)
    Trait,
    /// An enum: a marker on it claims its discriminant literals.
    // trace:TASK-1954 | ai:claude — TASK-1812 (7)
    Enum,
    Const,
    Static,
}

/// One item that can be a traced region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TracedItem {
    pub(crate) kind: ItemKind,
    /// `name` for a free fn, `Owner::name` for a method, trait default
    /// method or fn nested in another fn (the full owner path, e.g.
    /// `Type::method::inner`); empty for impls, traits and enums.
    pub(crate) name: String,
    /// 0-based, end-exclusive line span (attributes and doc comments
    /// included; the plain-comment prefix is not).
    pub(crate) range: Range<usize>,
    /// Marker ids this item owns (after the caller's filter).
    pub(crate) specs: BTreeSet<String>,
    /// Index in `TracedFile::items` of the item this one is nested in.
    // trace:TASK-1954 | ai:claude — TASK-1916 (2)
    pub(crate) parent: Option<usize>,
}

/// The trace regions of one Rust file.
#[derive(Debug, Clone, Default)]
pub(crate) struct TracedFile {
    /// Ids from module-level `//!` anchors: they claim every line.
    pub(crate) file_specs: BTreeSet<String>,
    pub(crate) items: Vec<TracedItem>,
    /// `rust_comments` output: comment text per 0-based line.
    pub(crate) comments: Vec<(usize, String)>,
    /// 0-based line spans of test-only code: `#[cfg(test)]` modules and
    /// items, and `#[test]` functions. Both lint constants and tree coverage
    /// exempt them, as SPIKE-47 exempts `tests/` files.
    // trace:TASK-1954 | ai:claude — TASK-1812, TASK-1916 (3)
    pub(crate) tests: Vec<Range<usize>>,
}

impl TracedFile {
    /// Every id claiming 0-based `line`: the file anchors plus every item
    /// whose span contains it. Line-granular: a marker claims every item on
    /// its line, so two items sharing one line share their markers.
    pub(crate) fn specs_at(&self, line: usize) -> BTreeSet<String> {
        let mut out = self.file_specs.clone();
        for item in self.items.iter().filter(|i| i.range.contains(&line)) {
            out.extend(item.specs.iter().cloned());
        }
        out
    }

    /// Every id claiming item `idx`: the file anchors, its own markers and
    /// those of the items it is nested in. Unlike `specs_at(range.start)`, a
    /// sibling that merely starts on the same line lends it nothing.
    // trace:TASK-1954 | ai:claude — TASK-1916 (2)
    pub(crate) fn item_specs(&self, idx: usize) -> BTreeSet<String> {
        let mut out = self.file_specs.clone();
        let mut cur = Some(idx);
        while let Some(i) = cur {
            let Some(item) = self.items.get(i) else {
                break;
            };
            out.extend(item.specs.iter().cloned());
            cur = item.parent;
        }
        out
    }

    /// True when 0-based `line` lies in test-only code.
    // trace:TASK-1954 | ai:claude — TASK-1812
    pub(crate) fn in_test(&self, line: usize) -> bool {
        self.tests.iter().any(|r| r.contains(&line))
    }
}

/// Raw item: kind, name, span, the line its marker ownership ends at, parent.
type RawItem = (ItemKind, String, Range<usize>, usize, Option<usize>);

/// Item spans in source order, with the line their marker ownership ends at.
#[derive(Default)]
struct Items {
    owner: Vec<String>,
    parents: Vec<usize>,
    out: Vec<RawItem>,
    tests: Vec<Range<usize>>,
}

fn line_range(span: proc_macro2::Span) -> Range<usize> {
    span.start().line.saturating_sub(1)..span.end().line
}

/// `cfg(test)` or `cfg(all(test, ...))`: the item compiles only under test.
/// `cfg(not(test))` and `cfg(any(test, ...))` do not count.
fn cfg_requires_test(tokens: proc_macro2::TokenStream) -> bool {
    let trees: Vec<proc_macro2::TokenTree> = tokens.into_iter().collect();
    match trees.as_slice() {
        [proc_macro2::TokenTree::Ident(i)] => i == "test",
        [proc_macro2::TokenTree::Ident(all), proc_macro2::TokenTree::Group(g)] if all == "all" => {
            let inner: Vec<proc_macro2::TokenTree> = g.stream().into_iter().collect();
            inner.iter().enumerate().any(|(n, t)| {
                matches!(t, proc_macro2::TokenTree::Ident(i) if i == "test")
                    && !matches!(inner.get(n + 1), Some(proc_macro2::TokenTree::Group(_)))
            })
        }
        _ => false,
    }
}

/// True for `#[test]` (any `...::test` path) and `#[cfg(test)]` attributes.
// trace:TASK-1954 | ai:claude — TASK-1812
fn is_test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        if a.path().segments.last().is_some_and(|s| s.ident == "test") {
            return true;
        }
        match &a.meta {
            syn::Meta::List(l) if l.path.is_ident("cfg") => cfg_requires_test(l.tokens.clone()),
            _ => false,
        }
    })
}

impl Items {
    fn push(
        &mut self,
        kind: ItemKind,
        ident: Option<&syn::Ident>,
        span: proc_macro2::Span,
    ) -> usize {
        let name = match ident {
            Some(i) if self.owner.is_empty() => i.to_string(),
            Some(i) => format!("{}::{i}", self.owner.join("::")),
            None => String::new(),
        };
        let range = line_range(span);
        let end = range.end;
        self.out
            .push((kind, name, range, end, self.parents.last().copied()));
        self.out.len() - 1
    }

    /// Visit an item's children with `idx` as their parent and, when given,
    /// `owner` appended to the naming path.
    fn nested(&mut self, idx: usize, owner: Option<String>, visit: impl FnOnce(&mut Self)) {
        self.parents.push(idx);
        let named = owner.is_some();
        if let Some(o) = owner {
            self.owner.push(o);
        }
        visit(self);
        if named {
            self.owner.pop();
        }
        self.parents.pop();
    }

    fn mark_test(&mut self, attrs: &[syn::Attribute], span: proc_macro2::Span) {
        if is_test_only(attrs) {
            self.tests.push(line_range(span));
        }
    }

    /// A block-scoped item owns only markers up to its `{` line.
    fn end_markers_at_brace(&mut self, idx: usize, brace: proc_macro2::Span) {
        self.out[idx].3 = brace.start().line;
    }
}

/// Last path segment of an impl's self type (`Foo` for `impl<T> a::Foo<T>`).
fn type_name(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_else(|| "_".into()),
        syn::Type::Reference(r) => type_name(&r.elem),
        _ => "_".into(),
    }
}

impl<'ast> Visit<'ast> for Items {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        self.mark_test(&node.attrs, node.span());
        syn::visit::visit_item_mod(self, node);
    }
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.mark_test(&node.attrs, node.span());
        let idx = self.push(ItemKind::Fn, Some(&node.sig.ident), node.span());
        self.nested(idx, Some(node.sig.ident.to_string()), |s| {
            syn::visit::visit_item_fn(s, node)
        });
    }
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        self.mark_test(&node.attrs, node.span());
        let idx = self.push(ItemKind::Impl, None, node.span());
        self.end_markers_at_brace(idx, node.brace_token.span.open());
        self.nested(idx, Some(type_name(&node.self_ty)), |s| {
            syn::visit::visit_item_impl(s, node)
        });
    }
    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.mark_test(&node.attrs, node.span());
        let idx = self.push(ItemKind::Fn, Some(&node.sig.ident), node.span());
        self.nested(idx, Some(node.sig.ident.to_string()), |s| {
            syn::visit::visit_impl_item_fn(s, node)
        });
    }
    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        self.mark_test(&node.attrs, node.span());
        let idx = self.push(ItemKind::Trait, None, node.span());
        self.end_markers_at_brace(idx, node.brace_token.span.open());
        self.nested(idx, Some(node.ident.to_string()), |s| {
            syn::visit::visit_item_trait(s, node)
        });
    }
    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        // A bodiless trait method declares, it does not implement.
        if node.default.is_none() {
            return syn::visit::visit_trait_item_fn(self, node);
        }
        let idx = self.push(ItemKind::Fn, Some(&node.sig.ident), node.span());
        self.nested(idx, Some(node.sig.ident.to_string()), |s| {
            syn::visit::visit_trait_item_fn(s, node)
        });
    }
    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        self.mark_test(&node.attrs, node.span());
        let idx = self.push(ItemKind::Enum, None, node.span());
        self.end_markers_at_brace(idx, node.brace_token.span.open());
        self.nested(idx, None, |s| syn::visit::visit_item_enum(s, node));
    }
    fn visit_item_const(&mut self, node: &'ast syn::ItemConst) {
        self.mark_test(&node.attrs, node.span());
        let idx = self.push(ItemKind::Const, Some(&node.ident), node.span());
        self.nested(idx, None, |s| syn::visit::visit_item_const(s, node));
    }
    fn visit_impl_item_const(&mut self, node: &'ast syn::ImplItemConst) {
        let idx = self.push(ItemKind::Const, Some(&node.ident), node.span());
        self.nested(idx, None, |s| syn::visit::visit_impl_item_const(s, node));
    }
    fn visit_item_static(&mut self, node: &'ast syn::ItemStatic) {
        self.mark_test(&node.attrs, node.span());
        let idx = self.push(ItemKind::Static, Some(&node.ident), node.span());
        self.nested(idx, None, |s| syn::visit::visit_item_static(s, node));
    }
}

/// The PURE core: parse `text` and attribute its trace markers to file,
/// impl and item scopes. `keep(id)` filters marker ids (e.g. to live specs);
/// ids are upper-cased first. `None` when the text does not parse.
pub(crate) fn traced_file<F>(text: &str, mut keep: F) -> Option<TracedFile>
where
    F: FnMut(&str) -> bool,
{
    let parsed = syn::parse_file(text).ok()?;
    let comments = crate::gate::mandate_traces::rust_comments(text)?;
    let lines: Vec<&str> = text.lines().collect();
    let anchor = crate::trace::coverage_anchor_re();
    let mut ids = |c: &str| -> Vec<String> {
        anchor
            .captures_iter(c)
            .map(|cap| cap[1].to_ascii_uppercase())
            .filter(|id| keep(id))
            .collect()
    };

    let file_specs: BTreeSet<String> = comments
        .iter()
        .zip(&lines)
        .filter(|(_, l)| l.trim_start().starts_with("//!"))
        .flat_map(|((_, c), _)| ids(c))
        .collect();

    let mut items = Items::default();
    items.visit_file(&parsed);
    let tests = items.tests;
    let traced_items = items
        .out
        .into_iter()
        .map(|(kind, name, range, marker_end, parent)| {
            let start =
                crate::gate::mandate_traces::leading_comment_start(range.start, &lines, &comments);
            let specs = (start..marker_end)
                .filter_map(|i| comments.get(i))
                .flat_map(|(_, c)| ids(c))
                .collect();
            TracedItem {
                kind,
                name,
                range,
                specs,
                parent,
            }
        })
        .collect();
    Some(TracedFile {
        file_specs,
        items: traced_items,
        comments,
        tests,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specs(v: &[&str]) -> BTreeSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // trace:TASK-1783.ac438744 | ai:claude
    #[test]
    fn file_scope_claims_every_line() {
        let src = "\
//! Module. trace:STORY-1 | ai:claude

fn a() {}
";
        let tf = traced_file(src, |_| true).unwrap();
        assert_eq!(tf.file_specs, specs(&["STORY-1"]));
        assert_eq!(tf.specs_at(2), specs(&["STORY-1"]));
    }

    // trace:TASK-1783.ac438744 | ai:claude
    #[test]
    fn impl_scope_claims_methods_but_a_method_marker_stays_local() {
        let src = "\
// trace:STORY-1 | ai:claude
impl A {
    fn a1(&self) {}
}
impl B {
    // trace:STORY-2 | ai:claude
    fn b1(&self) {}
    fn b2(&self) {}
}
";
        let tf = traced_file(src, |_| true).unwrap();
        assert_eq!(tf.specs_at(2), specs(&["STORY-1"]));
        assert_eq!(tf.specs_at(6), specs(&["STORY-2"]));
        assert!(tf.specs_at(7).is_empty());
        let names: Vec<&str> = tf.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["", "A::a1", "", "B::b1", "B::b2"]);
    }

    // trace:TASK-1783.ac438744 | ai:claude
    #[test]
    fn item_scope_takes_markers_inside_and_in_the_leading_prefix() {
        let src = "\
// trace:STORY-1 | ai:claude

fn across_blank() {}
fn after_code() {}
fn inside() {
    // trace:STORY-2 | ai:claude
}
const C: u8 = 2; // trace:STORY-3 | ai:claude
fn string() { let _ = \"trace:STORY-4\"; }
";
        let tf = traced_file(src, |_| true).unwrap();
        assert_eq!(tf.specs_at(2), specs(&["STORY-1"]));
        assert!(tf.specs_at(3).is_empty());
        assert_eq!(tf.specs_at(4), specs(&["STORY-2"]));
        assert_eq!(tf.specs_at(7), specs(&["STORY-3"]));
        assert!(tf.specs_at(8).is_empty());
    }

    #[test]
    fn filter_drops_ids_and_unparsable_text_is_none() {
        let src = "\
// trace:STORY-404 | ai:claude
fn f() {}
";
        let tf = traced_file(src, |id| id != "STORY-404").unwrap();
        assert!(tf.specs_at(1).is_empty());
        assert!(traced_file("fn broken( {", |_| true).is_none());
    }

    // trace:TASK-1954 | ai:claude — TASK-1812 (7)
    #[test]
    fn trait_and_enum_markers_claim_their_bodies() {
        let src = "\
// trace:STORY-1 | ai:claude
trait T {
    fn d(&self) -> u32 { 40 }
}
// trace:STORY-2 | ai:claude
enum E {
    A = 41,
}
";
        let tf = traced_file(src, |_| true).unwrap();
        assert_eq!(tf.specs_at(2), specs(&["STORY-1"]));
        assert_eq!(tf.specs_at(6), specs(&["STORY-2"]));
    }

    // trace:TASK-1954 | ai:claude — TASK-1812, TASK-1916 (3)
    #[test]
    fn cfg_test_modules_and_test_fns_are_test_spans() {
        let src = "\
fn prod() {}
#[cfg(test)]
mod tests {
    fn helper() {}
}
#[test]
fn t() {}
#[cfg(all(test, unix))]
fn u() {}
#[cfg(not(test))]
fn v() {}
";
        let tf = traced_file(src, |_| true).unwrap();
        let in_test: Vec<bool> = (0..11).map(|l| tf.in_test(l)).collect();
        assert_eq!(
            in_test,
            vec![false, true, true, true, true, true, true, true, true, false, false]
        );
    }

    // trace:TASK-1954 | ai:claude — TASK-1916 (2)
    #[test]
    fn item_specs_follow_nesting_not_shared_lines() {
        let src = "\
fn a() {} fn b() {
    // trace:STORY-1 | ai:claude
}
impl S {
    fn m(&self) {
        // trace:STORY-2 | ai:claude
        fn inner() {}
    }
}
";
        let tf = traced_file(src, |_| true).unwrap();
        let by_name = |n: &str| tf.items.iter().position(|i| i.name == n).unwrap();
        assert!(tf.item_specs(by_name("a")).is_empty());
        assert_eq!(tf.item_specs(by_name("b")), specs(&["STORY-1"]));
        assert_eq!(tf.item_specs(by_name("S::m::inner")), specs(&["STORY-2"]));
    }
}
