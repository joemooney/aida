//! Which lines of a Rust file a `trace:<SPEC-ID>` marker claims: the single
//! region rule shared by `aida lint constants` and whole-tree
//! `aida trace coverage`, so "traced" means one thing in both.
//!
//! - File scope: a module-level `//!` anchor claims every line of the file
//!   (the diff mode's FileAnchor rule).
//! - Impl scope: a marker in an `impl` block's leading prefix or header (up
//!   to its `{` line) claims the whole block, so all of its methods. A marker
//!   inside one method never claims its siblings.
//! - Item scope: a marker inside a function, method, trait default method,
//!   const or static, or in its leading comment/blank prefix (attached with
//!   the mandate-evidence walk), claims that item.
// trace:TASK-1783 | ai:claude

use std::collections::BTreeSet;
use std::ops::Range;

use syn::{spanned::Spanned, visit::Visit};

/// What kind of item a region is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemKind {
    Fn,
    Impl,
    Const,
    Static,
}

/// One item that can be a traced region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TracedItem {
    pub(crate) kind: ItemKind,
    /// `name`, `Type::name` for methods, `Trait::name` for trait defaults;
    /// empty for impls.
    pub(crate) name: String,
    /// 0-based, end-exclusive line span (attributes and doc comments
    /// included; the plain-comment prefix is not).
    pub(crate) range: Range<usize>,
    /// Marker ids this item owns (after the caller's filter).
    pub(crate) specs: BTreeSet<String>,
}

/// The trace regions of one Rust file.
#[derive(Debug, Clone, Default)]
pub(crate) struct TracedFile {
    /// Ids from module-level `//!` anchors: they claim every line.
    pub(crate) file_specs: BTreeSet<String>,
    pub(crate) items: Vec<TracedItem>,
    /// `rust_comments` output: comment text per 0-based line.
    pub(crate) comments: Vec<(usize, String)>,
}

impl TracedFile {
    /// Every id claiming 0-based `line`: the file anchors plus every item
    /// whose span contains it.
    pub(crate) fn specs_at(&self, line: usize) -> BTreeSet<String> {
        let mut out = self.file_specs.clone();
        for item in self.items.iter().filter(|i| i.range.contains(&line)) {
            out.extend(item.specs.iter().cloned());
        }
        out
    }
}

/// Item spans in source order, with the line their marker ownership ends at.
#[derive(Default)]
struct Items {
    owner: Vec<String>,
    out: Vec<(ItemKind, String, Range<usize>, usize)>,
}

impl Items {
    fn push(&mut self, kind: ItemKind, ident: Option<&syn::Ident>, span: proc_macro2::Span) {
        let name = match (ident, self.owner.last()) {
            (Some(i), Some(owner)) => format!("{owner}::{i}"),
            (Some(i), None) => i.to_string(),
            (None, _) => String::new(),
        };
        let range = span.start().line.saturating_sub(1)..span.end().line;
        let end = range.end;
        self.out.push((kind, name, range, end));
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
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.push(ItemKind::Fn, Some(&node.sig.ident), node.span());
        syn::visit::visit_item_fn(self, node);
    }
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        self.push(ItemKind::Impl, None, node.span());
        // An impl owns only markers up to its `{` line.
        if let Some(last) = self.out.last_mut() {
            last.3 = node.brace_token.span.open().start().line;
        }
        self.owner.push(type_name(&node.self_ty));
        syn::visit::visit_item_impl(self, node);
        self.owner.pop();
    }
    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.push(ItemKind::Fn, Some(&node.sig.ident), node.span());
        syn::visit::visit_impl_item_fn(self, node);
    }
    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        self.owner.push(node.ident.to_string());
        syn::visit::visit_item_trait(self, node);
        self.owner.pop();
    }
    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        // A bodiless trait method declares, it does not implement.
        if node.default.is_some() {
            self.push(ItemKind::Fn, Some(&node.sig.ident), node.span());
        }
        syn::visit::visit_trait_item_fn(self, node);
    }
    fn visit_item_const(&mut self, node: &'ast syn::ItemConst) {
        self.push(ItemKind::Const, Some(&node.ident), node.span());
        syn::visit::visit_item_const(self, node);
    }
    fn visit_impl_item_const(&mut self, node: &'ast syn::ImplItemConst) {
        self.push(ItemKind::Const, Some(&node.ident), node.span());
        syn::visit::visit_impl_item_const(self, node);
    }
    fn visit_item_static(&mut self, node: &'ast syn::ItemStatic) {
        self.push(ItemKind::Static, Some(&node.ident), node.span());
        syn::visit::visit_item_static(self, node);
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
    let items = items
        .out
        .into_iter()
        .map(|(kind, name, range, marker_end)| {
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
            }
        })
        .collect();
    Some(TracedFile {
        file_specs,
        items,
        comments,
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
}
