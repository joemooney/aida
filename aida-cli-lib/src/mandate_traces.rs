//! Committed trace evidence for mandate EXCLUSIONS only. No completion or
//! author/reviewer authority may be derived from this module.
// trace:TASK-1790 | ai:codex

use anyhow::{ensure, Context, Result};
use std::{collections::BTreeSet, path::Path};
use syn::{spanned::Spanned, visit::Visit};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Scope {
    // The full-file baseline is retained for the required historical benchmark.
    #[allow(dead_code)]
    FullFile,
    ChangedItems,
    /// The TASK-1790 fn/impl attribution without the AC2 reference closure,
    /// kept for the corpus replay's before/after comparison.
    #[cfg(test)]
    ChangedItemsV1,
}

#[derive(Debug, Default)]
pub(crate) struct Evidence {
    pub ids: BTreeSet<String>,
    /// Paths for which an item could not safely be attributed. Full-file
    /// union is the conservative fallback, never an empty evidence set.
    pub full_file_fallbacks: BTreeSet<String>,
    /// STORY-1700 (R1): every path the pinned diff changes (both names of a
    /// rename), for the mandate's gate-touching check.
    // trace:STORY-1700 | ai:claude
    pub paths: BTreeSet<String>,
    /// Per changed path, both sides: the IDs it supplied and why it fell back.
    /// Replay provenance for the TASK-1800 corpus test only.
    #[cfg(test)]
    pub per_path: std::collections::BTreeMap<String, (BTreeSet<String>, BTreeSet<Fallback>)>,
}

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    // Store status can execute fsmonitor: use the shared host boundary for
    // every command, including read-only source/object evidence.
    let out = aida_core::git_ops::host_git_command(repo)
        .args(args)
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .output()
        .context("reading committed mandate trace evidence")?;
    ensure!(
        out.status.success(),
        "git {}: {}",
        args[0],
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(out.stdout)
}

fn oid(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Pin the merge base against one locally available, forge-confirmed base
/// branch. No fallback to an unrelated local branch or empty diff is permitted.
pub(crate) fn collect_pr(
    repo: &Path,
    base_branch: &str,
    expected_head: &str,
    forge_head: &str,
) -> Result<Evidence> {
    ensure!(
        oid(expected_head) && expected_head.eq_ignore_ascii_case(forge_head),
        "PR head changed or could not be pinned for mandate exclusions"
    );
    let base_ref = format!("refs/remotes/origin/{base_branch}");
    git(repo, &["check-ref-format", &base_ref])?;
    let tip = git(
        repo,
        &["rev-parse", "--verify", &format!("{base_ref}^{{commit}}")],
    )?;
    let tip = std::str::from_utf8(&tip)?.trim();
    ensure!(oid(tip), "unreadable mandate base tip");
    let bases = git(repo, &["merge-base", "--all", tip, expected_head])?;
    let bases = std::str::from_utf8(&bases)?;
    let bases: Vec<_> = bases.lines().collect();
    ensure!(
        bases.len() == 1 && oid(bases[0]),
        "mandate change has no unique merge base"
    );
    collect(repo, bases[0], expected_head, Scope::ChangedItems)
}

/// Refuse a moving or dirty store rather than mixing uncommitted working objects
/// with a historical exclusion read. Ref tips bind the existing --all history
/// reader, including sticky versions not reachable from the store's HEAD.
pub(crate) fn store_snapshot(store: &Path) -> Result<Vec<u8>> {
    ensure!(
        git(
            store,
            &["status", "--porcelain=v1", "--untracked-files=all"]
        )?
        .is_empty(),
        "mandate exclusion store has uncommitted evidence; retry after the store write completes"
    );
    let mut snapshot = git(store, &["rev-parse", "--verify", "HEAD"])?;
    snapshot.extend(git(store, &["show-ref", "--head"])?);
    Ok(snapshot)
}

/// Inputs must already be pinned commit IDs. Paths are decoded only for source
/// classification/diagnostics; blobs are read by object ID, never via a checkout,
/// worktree symlink, textconv, external diff or an interpolated revision:path.
/// Renames are deliberately represented as deletion + addition, retaining BOTH
/// names and versions, including a rename across the source-extension boundary.
pub(crate) fn collect(repo: &Path, base: &str, head: &str, scope: Scope) -> Result<Evidence> {
    ensure!(
        oid(base) && oid(head),
        "mandate trace evidence requires pinned commit IDs"
    );
    for commit in [base, head] {
        let ty = git(repo, &["cat-file", "-t", commit])?;
        ensure!(ty == b"commit\n", "mandate trace endpoint is not a commit");
    }
    let raw = git(
        repo,
        &[
            "diff-tree",
            "--raw",
            "-z",
            "-r",
            "--no-abbrev",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=none",
            base,
            head,
            "--",
        ],
    )?;
    let mut fields = raw.split(|b| *b == 0);
    let mut out = Evidence::default();
    while let Some(header) = fields.next() {
        if header.is_empty() {
            ensure!(fields.next().is_none(), "invalid trailing diff evidence");
            break;
        }
        let header = std::str::from_utf8(header).context("non-text diff header")?;
        let words: Vec<_> = header
            .strip_prefix(':')
            .context("invalid raw diff header")?
            .split_whitespace()
            .collect();
        ensure!(
            words.len() == 5 && matches!(words[4], "A" | "D" | "M" | "T"),
            "unsupported raw diff evidence"
        );
        let path = std::str::from_utf8(fields.next().context("missing raw diff path")?)
            .context("non-UTF8 changed path")?;
        out.paths.insert(path.to_owned());
        let mut sides = Vec::new();
        for (mode, id) in [(words[0], words[2]), (words[1], words[3])] {
            ensure!(oid(id), "invalid blob ID for {path}");
            if mode == "000000" {
                ensure!(
                    id.bytes().all(|b| b == b'0'),
                    "inconsistent absent side for {path}"
                );
                sides.push(None);
            } else {
                ensure!(
                    matches!(mode, "100644" | "100755"),
                    "changed source {path} is not an ordinary regular file ({mode})"
                );
                let bytes = git(repo, &["cat-file", "blob", id])
                    .with_context(|| format!("reading {path} blob {id}"))?;
                ensure!(!bytes.contains(&0), "binary source evidence for {path}");
                sides.push(Some(
                    String::from_utf8(bytes).with_context(|| format!("non-UTF8 source {path}"))?,
                ));
            }
        }
        let old = sides[0].as_deref().unwrap_or("");
        let new = sides[1].as_deref().unwrap_or("");
        // syn spans and the comment map count LF lines; similar's default
        // line tokenizer also splits bare CR, which can hide a guarded item.
        let old_lines: Vec<_> = old.split_inclusive('\n').collect();
        let new_lines: Vec<_> = new.split_inclusive('\n').collect();
        let diff = similar::TextDiff::from_slices(&old_lines, &new_lines);
        for (index, text) in sides.iter().enumerate() {
            let Some(text) = text else { continue };
            let ranges: Vec<_> = diff
                .ops()
                .iter()
                .filter(|op| op.tag() != similar::DiffTag::Equal)
                .map(|op| {
                    if index == 0 {
                        op.old_range()
                    } else {
                        op.new_range()
                    }
                })
                .collect();
            let (ids, fallback) = source_evidence(path, text, &ranges, scope);
            #[cfg(test)]
            {
                let entry = out.per_path.entry(path.to_owned()).or_default();
                entry.0.extend(ids.iter().cloned());
                entry.1.extend(fallback);
            }
            out.ids.extend(ids);
            if fallback.is_some() {
                out.full_file_fallbacks.insert(path.to_owned());
            }
        }
    }
    Ok(out)
}

fn trace_ids(text: &str) -> BTreeSet<String> {
    // Strip criterion suffixes: exclusions belong to the owning spec, not ACs.
    // Multiple IDs in a trace run and the existing qualifier forms are retained.
    static ID: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let id = ID.get_or_init(|| {
        regex::Regex::new(r"(?i)^([a-z][a-z0-9]*(?:-[a-z0-9]+)*-[0-9]+)(?:$|[^a-z0-9_-])")
            .expect("valid mandate trace ID regex")
    });
    let mut out = BTreeSet::new();
    static MARKER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let marker = MARKER.get_or_init(|| regex::Regex::new("(?i)trace:").unwrap());
    for run in marker.split(text).skip(1) {
        for token in run.split(|c: char| c.is_whitespace() || c == ',') {
            if token.is_empty() {
                continue;
            }
            let candidate = token.rsplit(':').next().unwrap_or(token);
            if let Some(captures) = id.captures(candidate) {
                out.insert(captures[1].to_ascii_uppercase());
            } else {
                break;
            }
        }
    }
    out
}

/// Token spans let us distinguish actual Rust comments from strings, including
/// raw strings and test fixtures. Rust doc-comment tokens carry the ORIGINAL
/// comment span; retain those rather than masking their synthesized #[doc].
/// If tokenization fails, raw full-file tokens are conservative fallback evidence.
pub(crate) fn rust_comments(text: &str) -> Option<Vec<(usize, String)>> {
    let stream: proc_macro2::TokenStream = text.parse().ok()?;
    let mut offsets = vec![0];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            offsets.push(i + 1);
        }
    }
    // proc-macro columns count Unicode scalar values, not UTF-8 bytes.
    fn byte_at(text: &str, offsets: &[usize], p: proc_macro2::LineColumn) -> Option<usize> {
        let start = *offsets.get(p.line.checked_sub(1)?)?;
        let rest = &text[start..];
        Some(
            start
                + rest
                    .char_indices()
                    .nth(p.column)
                    .map(|(i, _)| i)
                    .unwrap_or(rest.len()),
        )
    }
    fn spans(
        stream: proc_macro2::TokenStream,
        out: &mut Vec<proc_macro2::Span>,
        text: &str,
        offsets: &[usize],
    ) -> Option<()> {
        for tree in stream {
            let span = tree.span();
            let original = text
                .get(byte_at(text, offsets, span.start())?..byte_at(text, offsets, span.end())?)?;
            if original.starts_with("//") || original.starts_with("/*") {
                continue;
            }
            if let proc_macro2::TokenTree::Group(g) = tree {
                if g.delimiter() != proc_macro2::Delimiter::None {
                    out.push(g.span_open());
                    out.push(g.span_close());
                }
                spans(g.stream(), out, text, offsets)?;
            } else {
                out.push(span);
            }
        }
        Some(())
    }
    let mut token_spans = Vec::new();
    spans(stream, &mut token_spans, text, &offsets)?;
    let mut mask = vec![false; text.len()];
    for span in token_spans {
        let start = byte_at(text, &offsets, span.start())?;
        let end = byte_at(text, &offsets, span.end())?;
        let original = text.get(start..end)?;
        if original.starts_with("//") || original.starts_with("/*") {
            continue;
        }
        mask.get_mut(start..end)?.fill(true);
    }
    // Unmasked non-whitespace bytes are comments. Preserve line numbers and
    // retain comment text only; strings and punctuation are all masked.
    Some(
        text.lines()
            .enumerate()
            .map(|(line, s)| {
                let start = offsets[line];
                let comment: String = s
                    .char_indices()
                    .map(|(i, c)| if mask[start + i] { ' ' } else { c })
                    .collect();
                (line, comment)
            })
            .collect(),
    )
}

/// Plain leading comments are not part of syn's item span. Extend an item
/// starting at 0-based line `start` over its uninterrupted comment/blank
/// prefix; `comments` is `rust_comments` output for the same text.
// trace:TASK-1785 | ai:claude — shared with `aida lint constants`.
pub(crate) fn leading_comment_start(
    mut start: usize,
    lines: &[&str],
    comments: &[(usize, String)],
) -> usize {
    while start > 0 {
        let i = start - 1;
        if lines[i].trim().is_empty()
            || (!comments[i].1.trim().is_empty() && lines[i].trim() == comments[i].1.trim())
        {
            start -= 1;
        } else {
            break;
        }
    }
    start
}

/// One attribution unit: a syn item of any kind, nested items included, with
/// its leading comment/blank prefix. An inline module is only its shell (the
/// lines through its open brace, and its close-brace line); the items inside
/// it are units of their own.
// trace:TASK-1800 | ai:claude
struct Unit {
    ranges: Vec<std::ops::Range<usize>>,
    /// Names the unit defines, for the same-file reference closure: the item
    /// ident, struct/union field and enum variant names, the item names in an
    /// impl, trait or foreign block, the names a `use` brings in.
    names: BTreeSet<String>,
    /// The closure cannot see what this unit expands to or brings in: an item
    /// macro or `macro_rules!`, an item syn kept unparsed, or a glob import.
    opaque: bool,
    /// fn, impl and impl fn: the TASK-1790 unit set. Module-comment ownership
    /// is still decided over this set alone.
    v1: bool,
    /// A `use`: changing it can change trait-mediated method resolution (a
    /// trait import, `as _`) that no name mention shows, so it fails closed.
    import: bool,
}

#[derive(Default)]
struct Items {
    units: Vec<Unit>,
    module_opens: BTreeSet<(usize, usize)>,
    /// Each macro invocation: its name and every identifier in its tokens.
    macro_calls: Vec<(String, BTreeSet<String>)>,
    /// Names this file defines with `macro_rules!`.
    local_macros: BTreeSet<String>,
    /// Lines of every non-doc attribute. A derive or attribute macro expands
    /// to code the closure cannot see, so a change on them fails closed.
    attr_lines: Vec<std::ops::Range<usize>>,
    /// Source the parse of this file cannot see: `include!`, or an out-of-line
    /// module (`mod m;`, `#[path]`, `#[cfg_attr(.., path = ..)]`) that is not
    /// test-only.
    external_source: bool,
}

/// std/core/alloc macros. They only expand their arguments, whose names the
/// closure already sees; any other macro may be crate-local and count.
// trace:TASK-1800 | ai:claude
const STD_MACROS: &[&str] = &[
    "assert",
    "assert_eq",
    "assert_ne",
    "cfg",
    "column",
    "compile_error",
    "concat",
    "dbg",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "env",
    "eprint",
    "eprintln",
    "file",
    "format",
    "format_args",
    "line",
    "matches",
    "module_path",
    "option_env",
    "panic",
    "print",
    "println",
    "stringify",
    "todo",
    "unimplemented",
    "unreachable",
    "vec",
    "write",
    "writeln",
];

/// `#[cfg(test)]` exactly: a test-only module carries no production code.
fn cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && matches!(&a.meta, syn::Meta::List(l) if l.tokens.to_string() == "test")
    })
}

/// `#[path = ..]`, or `path` anywhere inside a `#[cfg_attr(..)]`.
fn path_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("path")
            || (a.path().is_ident("cfg_attr")
                && matches!(&a.meta, syn::Meta::List(l) if {
                    let mut idents = BTreeSet::new();
                    macro_idents(l.tokens.clone(), &mut idents);
                    idents.contains("path")
                }))
    })
}

fn name(ident: &syn::Ident) -> String {
    let s = ident.to_string();
    s.strip_prefix("r#").map_or(s.clone(), str::to_owned)
}

fn field_names(fields: &syn::Fields, names: &mut BTreeSet<String>) {
    names.extend(fields.iter().filter_map(|f| f.ident.as_ref()).map(name));
}

/// The names a `use` tree brings in; `true` when it globs, whose names the
/// closure cannot know.
fn use_names(
    tree: &syn::UseTree,
    parent: Option<&syn::Ident>,
    names: &mut BTreeSet<String>,
) -> bool {
    match tree {
        syn::UseTree::Path(p) => use_names(&p.tree, Some(&p.ident), names),
        syn::UseTree::Name(n) if n.ident == "self" => {
            let Some(parent) = parent else { return true };
            names.insert(name(parent));
            false
        }
        syn::UseTree::Name(n) => {
            names.insert(name(&n.ident));
            false
        }
        syn::UseTree::Rename(r) => {
            names.insert(name(&r.rename));
            false
        }
        syn::UseTree::Glob(_) => true,
        syn::UseTree::Group(g) => g
            .items
            .iter()
            .fold(false, |glob, t| use_names(t, parent, names) | glob),
    }
}

/// `(name, opaque)` for an item inside an impl, trait or foreign block.
fn impl_item(item: &syn::ImplItem) -> (Option<&syn::Ident>, bool) {
    match item {
        syn::ImplItem::Const(i) => (Some(&i.ident), false),
        syn::ImplItem::Fn(i) => (Some(&i.sig.ident), false),
        syn::ImplItem::Type(i) => (Some(&i.ident), false),
        _ => (None, true),
    }
}
fn trait_item(item: &syn::TraitItem) -> (Option<&syn::Ident>, bool) {
    match item {
        syn::TraitItem::Const(i) => (Some(&i.ident), false),
        syn::TraitItem::Fn(i) => (Some(&i.sig.ident), false),
        syn::TraitItem::Type(i) => (Some(&i.ident), false),
        _ => (None, true),
    }
}
fn foreign_item(item: &syn::ForeignItem) -> (Option<&syn::Ident>, bool) {
    match item {
        syn::ForeignItem::Fn(i) => (Some(&i.sig.ident), false),
        syn::ForeignItem::Static(i) => (Some(&i.ident), false),
        syn::ForeignItem::Type(i) => (Some(&i.ident), false),
        _ => (None, true),
    }
}

/// Add the member names of an impl, trait or foreign block; `true` when a
/// member is opaque.
fn members<'a>(
    items: impl Iterator<Item = (Option<&'a syn::Ident>, bool)>,
    names: &mut BTreeSet<String>,
) -> bool {
    let mut opaque = false;
    for (ident, hidden) in items {
        names.extend(ident.map(name));
        opaque |= hidden;
    }
    opaque
}

fn macro_idents(stream: proc_macro2::TokenStream, out: &mut BTreeSet<String>) {
    for tree in stream {
        match tree {
            proc_macro2::TokenTree::Ident(i) => {
                out.insert(name(&i));
            }
            proc_macro2::TokenTree::Group(g) => macro_idents(g.stream(), out),
            _ => {}
        }
    }
}

// trace:TASK-1800 | ai:claude
impl<'ast> Visit<'ast> for Items {
    fn visit_item(&mut self, node: &'ast syn::Item) {
        use syn::Item as I;
        let mut names = BTreeSet::new();
        let mut opaque = false;
        let mut shell = None;
        let mut own = |ident: &syn::Ident| {
            names.insert(name(ident));
        };
        match node {
            I::Fn(i) => own(&i.sig.ident),
            I::Const(i) => own(&i.ident),
            I::Static(i) => own(&i.ident),
            I::Type(i) => own(&i.ident),
            I::TraitAlias(i) => own(&i.ident),
            I::ExternCrate(i) => own(i.rename.as_ref().map_or(&i.ident, |(_, r)| r)),
            I::Macro(i) => {
                i.ident.iter().for_each(own);
                self.local_macros.extend(i.ident.iter().map(name));
                opaque = true;
            }
            I::Mod(i) => {
                own(&i.ident);
                // A test-only module is a unit like any other; any other
                // out-of-line or re-pathed module hides source.
                if (i.content.is_none() || path_attr(&i.attrs)) && !cfg_test(&i.attrs) {
                    self.external_source = true;
                }
                if let Some((brace, _)) = &i.content {
                    let open = brace.span.open().start();
                    self.module_opens.insert((open.line, open.column));
                    shell = Some((open.line, brace.span.close().end().line));
                }
            }
            I::Struct(i) => {
                own(&i.ident);
                field_names(&i.fields, &mut names);
            }
            I::Union(i) => {
                own(&i.ident);
                field_names(&syn::Fields::Named(i.fields.clone()), &mut names);
            }
            I::Enum(i) => {
                own(&i.ident);
                for variant in &i.variants {
                    names.insert(name(&variant.ident));
                    field_names(&variant.fields, &mut names);
                }
            }
            I::Impl(i) => opaque = members(i.items.iter().map(impl_item), &mut names),
            I::Trait(i) => {
                own(&i.ident);
                opaque = members(i.items.iter().map(trait_item), &mut names);
            }
            I::ForeignMod(i) => opaque = members(i.items.iter().map(foreign_item), &mut names),
            I::Use(i) => opaque = use_names(&i.tree, None, &mut names),
            // Verbatim and any kind syn adds later: nothing to attribute.
            _ => opaque = true,
        }
        let v1 = matches!(node, I::Fn(_) | I::Impl(_));
        let span = node.span();
        let start = span.start().line.saturating_sub(1);
        let ranges = match shell {
            Some((open, close)) => vec![start..open, close.saturating_sub(1)..close],
            None => vec![start..span.end().line],
        };
        self.units.push(Unit {
            ranges,
            names,
            opaque,
            v1,
            import: matches!(node, I::Use(_)),
        });
        syn::visit::visit_item(self, node);
    }
    fn visit_impl_item(&mut self, node: &'ast syn::ImplItem) {
        let (ident, opaque) = impl_item(node);
        let v1 = matches!(node, syn::ImplItem::Fn(_));
        self.push(node.span(), ident, opaque, v1);
        syn::visit::visit_impl_item(self, node);
    }
    fn visit_trait_item(&mut self, node: &'ast syn::TraitItem) {
        let (ident, opaque) = trait_item(node);
        self.push(node.span(), ident, opaque, false);
        syn::visit::visit_trait_item(self, node);
    }
    fn visit_foreign_item(&mut self, node: &'ast syn::ForeignItem) {
        let (ident, opaque) = foreign_item(node);
        self.push(node.span(), ident, opaque, false);
        syn::visit::visit_foreign_item(self, node);
    }
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        if node
            .path
            .segments
            .last()
            .is_some_and(|s| s.ident == "include")
        {
            self.external_source = true;
        }
        let mut idents = BTreeSet::new();
        macro_idents(node.tokens.clone(), &mut idents);
        let called = node.path.segments.last().map(|s| name(&s.ident));
        self.macro_calls.push((called.unwrap_or_default(), idents));
        syn::visit::visit_macro(self, node);
    }
    fn visit_attribute(&mut self, node: &'ast syn::Attribute) {
        if !node.path().is_ident("doc") {
            let span = node.span();
            self.attr_lines
                .push(span.start().line.saturating_sub(1)..span.end().line);
        }
        syn::visit::visit_attribute(self, node);
    }
}
impl Items {
    fn push(
        &mut self,
        span: proc_macro2::Span,
        ident: Option<&syn::Ident>,
        opaque: bool,
        v1: bool,
    ) {
        self.units.push(Unit {
            ranges: vec![span.start().line.saturating_sub(1)..span.end().line],
            names: ident.map(name).into_iter().collect(),
            opaque,
            v1,
            import: false,
        });
    }
}

// Skip attributes (including synthesized doc attributes) while retaining their
// original comment lines and the first code-token line. Whole-line inclusion
// preserves the established conservative same-line prefix behavior.
// trace:TASK-1800 | ai:codex
fn header_end(stream: proc_macro2::TokenStream, empty_end: usize) -> Option<usize> {
    let mut tokens = stream.into_iter().peekable();
    loop {
        match tokens.peek() {
            Some(proc_macro2::TokenTree::Punct(p)) if p.as_char() == '#' => {
                tokens.next();
                if matches!(tokens.peek(), Some(proc_macro2::TokenTree::Punct(p)) if p.as_char() == '!')
                {
                    tokens.next();
                }
                if !matches!(tokens.next(), Some(proc_macro2::TokenTree::Group(g)) if g.delimiter() == proc_macro2::Delimiter::Bracket)
                {
                    return None;
                }
            }
            Some(token) => return Some(token.span().start().line),
            None => return Some(empty_end),
        }
    }
}

// trace:TASK-1800 | ai:codex
fn select_module_headers(
    stream: proc_macro2::TokenStream,
    module_opens: &BTreeSet<(usize, usize)>,
    selected: &mut [bool],
) -> Option<()> {
    for tree in stream {
        if let proc_macro2::TokenTree::Group(group) = tree {
            let start = group.span_open().start();
            if group.delimiter() == proc_macro2::Delimiter::Brace
                && module_opens.contains(&(start.line, start.column))
            {
                let end = header_end(group.stream(), group.span_close().end().line)?;
                let start = start.line.checked_sub(1)?;
                let end = end.min(selected.len());
                selected.get_mut(start..end)?.fill(true);
            }
            select_module_headers(group.stream(), module_opens, selected)?;
        }
    }
    Some(())
}

/// Why a changed path fell back to its full-file union. Replay provenance
/// only; never an authority input.
// trace:TASK-1800 | ai:claude
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Fallback {
    /// No proven item parser for the language.
    NotRust,
    /// Tokenizing or parsing failed.
    Unparsed,
    /// A mode-only change has no text hunk to attribute.
    ModeOnly,
    /// A changed line is in no unit.
    Uncovered,
    /// A changed or selected unit holds an item macro, `macro_rules!`, an
    /// unparsed item or a glob import.
    Opaque,
    /// A closure name appears inside a non-std macro invocation.
    MacroName,
    /// A changed line is in a `use` (trait imports, `as _`).
    ChangedUse,
    /// A changed line is in a non-doc attribute (derive, attribute macros).
    ChangedAttribute,
    /// `include!` or a non-test out-of-line module is present.
    ExternalSource,
}

#[cfg(test)]
fn source_ids(
    path: &str,
    text: &str,
    changes: &[std::ops::Range<usize>],
    scope: Scope,
) -> (BTreeSet<String>, bool) {
    let (ids, fallback) = source_evidence(path, text, changes, scope);
    (ids, fallback.is_some())
}

fn source_evidence(
    path: &str,
    text: &str,
    changes: &[std::ops::Range<usize>],
    scope: Scope,
) -> (BTreeSet<String>, Option<Fallback>) {
    if !path.ends_with(".rs") {
        // No proven item/comment parser for other languages: full-file raw
        // union is conservative (may include strings), never guessed attribution.
        let fallback = (!matches!(scope, Scope::FullFile)).then_some(Fallback::NotRust);
        return (trace_ids(text), fallback);
    }
    let Some(comments) = rust_comments(text) else {
        return (trace_ids(text), Some(Fallback::Unparsed));
    };
    let full = || comments.iter().flat_map(|(_, c)| trace_ids(c)).collect();
    if matches!(scope, Scope::FullFile) {
        return (full(), None);
    }
    // A mode-only change has no text hunks to attribute; it is still a
    // changed source file and therefore requires the signed full-file fallback.
    if changes.is_empty() {
        return (full(), Some(Fallback::ModeOnly));
    }
    let Ok(file) = syn::parse_file(text) else {
        return (full(), Some(Fallback::Unparsed));
    };
    let mut items = Items::default();
    items.visit_file(&file);
    #[cfg(test)]
    let v1 = matches!(scope, Scope::ChangedItemsV1);
    #[cfg(not(test))]
    let v1 = false;
    if v1 {
        items.units.retain(|unit| unit.v1);
    }
    let lines: Vec<_> = text.lines().collect();
    for unit in &mut items.units {
        if let Some(first) = unit.ranges.first_mut() {
            first.start = leading_comment_start(first.start, &lines, &comments);
        }
    }
    // Select units first, then parse each selected comment ONCE. A new
    // large file must not rescan an entire impl for every changed source line.
    let mut selected = vec![false; items.units.len()];
    for change in changes {
        // Empty-side insertion/deletion boundaries are evidence too. Either
        // adjacent line must attribute to a unit; otherwise use the full file.
        let touched: Vec<usize> = if change.is_empty() {
            [
                change.start.checked_sub(1),
                (change.start < lines.len()).then_some(change.start),
            ]
            .into_iter()
            .flatten()
            .collect()
        } else {
            change.clone().collect()
        };
        for line in touched {
            let mut attributed = false;
            for (i, unit) in items.units.iter().enumerate() {
                if unit.ranges.iter().any(|r| r.contains(&line)) {
                    attributed = true;
                    selected[i] = true;
                }
            }
            if !attributed {
                return (full(), Some(Fallback::Uncovered));
            }
            // trace:TASK-1800 | ai:claude
            if !v1 {
                if items.attr_lines.iter().any(|r| r.contains(&line)) {
                    return (full(), Some(Fallback::ChangedAttribute));
                }
                let import = items
                    .units
                    .iter()
                    .any(|unit| unit.import && unit.ranges.iter().any(|r| r.contains(&line)));
                if import {
                    return (full(), Some(Fallback::ChangedUse));
                }
            }
        }
    }
    let tokens = text.parse::<proc_macro2::TokenStream>().unwrap();
    // TASK-1800 AC2: the same-file reference closure. Every unit that
    // mentions a name a selected unit defines is selected too, to a fixpoint;
    // a closure the parse cannot see fails closed to the full file.
    // trace:TASK-1800 | ai:claude
    if !v1 {
        if items.external_source {
            return (full(), Some(Fallback::ExternalSource));
        }
        if let Some(why) = reference_closure(&items, &tokens, lines.len(), &mut selected) {
            return (full(), Some(why));
        }
    }
    let mut selected_lines = vec![false; comments.len()];
    // Module header comments apply to every item in the file, even when a use,
    // type or other sibling separates them from the changed function. Inner
    // doc comments belong to the file attributes, before its first item too.
    // This only adds evidence; item/fallback attribution remains unchanged.
    // trace:TASK-1800 | ai:codex
    let Some(header_end) = header_end(tokens.clone(), comments.len()) else {
        return (full(), Some(Fallback::Unparsed));
    };
    selected_lines[..header_end.min(comments.len())].fill(true);
    // Comments not owned by any function/impl (including its leading prefix)
    // are module evidence, wherever they occur in the file. This is additive:
    // sibling item traces remain scoped and attribution fallback is unchanged.
    // Ownership stays decided over the fn/impl units alone, so widening the
    // unit set never removes module evidence.
    // trace:TASK-1800 | ai:codex
    let mut owned_lines = vec![false; comments.len()];
    for unit in items.units.iter().filter(|unit| unit.v1) {
        for range in &unit.ranges {
            owned_lines[range.start.min(comments.len())..range.end.min(comments.len())].fill(true);
        }
    }
    for (line, owned) in owned_lines.into_iter().enumerate() {
        selected_lines[line] |= !owned;
    }
    // A module header may be attached as a leading comment to its first fn;
    // it still applies independently of which function changed. Use the actual
    // syn module brace identities, never arbitrary function/macro brace groups.
    if select_module_headers(tokens, &items.module_opens, &mut selected_lines).is_none() {
        return (full(), Some(Fallback::Unparsed));
    }
    for (unit, _) in items.units.iter().zip(&selected).filter(|(_, s)| **s) {
        for range in &unit.ranges {
            selected_lines[range.start.min(comments.len())..range.end.min(comments.len())]
                .fill(true);
        }
    }
    let selected = comments
        .iter()
        .filter(|(line, _)| selected_lines[*line])
        .flat_map(|(_, comment)| trace_ids(comment))
        .collect();
    (selected, None)
}

/// Grow `selected` over every unit that mentions a name a selected unit
/// defines, to a fixpoint. `Some` when the closure cannot be trusted: a
/// selected unit holds or overlaps an opaque unit, or a closure name appears
/// inside a non-std macro invocation, whose expansion the parse cannot see.
// trace:TASK-1800 | ai:claude
fn reference_closure(
    items: &Items,
    tokens: &proc_macro2::TokenStream,
    line_count: usize,
    selected: &mut [bool],
) -> Option<Fallback> {
    fn idents(stream: proc_macro2::TokenStream, by_line: &mut [Vec<String>]) {
        for tree in stream {
            match tree {
                proc_macro2::TokenTree::Ident(i) => {
                    if let Some(line) = by_line.get_mut(i.span().start().line.saturating_sub(1)) {
                        line.push(name(&i));
                    }
                }
                proc_macro2::TokenTree::Group(g) => idents(g.stream(), by_line),
                _ => {}
            }
        }
    }
    let mut by_line = vec![Vec::new(); line_count + 1];
    idents(tokens.clone(), &mut by_line);
    // name -> the units whose lines mention it
    let mut mentioned_by: std::collections::BTreeMap<&str, Vec<usize>> = Default::default();
    for (i, unit) in items.units.iter().enumerate() {
        let mut mentions = BTreeSet::new();
        for range in &unit.ranges {
            for line in by_line[range.start.min(line_count)..range.end.min(line_count)].iter() {
                mentions.extend(line.iter().map(String::as_str));
            }
        }
        for mention in mentions {
            mentioned_by.entry(mention).or_default().push(i);
        }
    }
    let mut names: BTreeSet<&str> = BTreeSet::new();
    let mut queue: Vec<&str> = Vec::new();
    for (unit, _) in items.units.iter().zip(selected.iter()).filter(|(_, s)| **s) {
        for n in &unit.names {
            if names.insert(n) {
                queue.push(n);
            }
        }
    }
    while let Some(n) = queue.pop() {
        for &i in mentioned_by.get(n).into_iter().flatten() {
            if std::mem::replace(&mut selected[i], true) {
                continue;
            }
            for n in &items.units[i].names {
                if names.insert(n) {
                    queue.push(n);
                }
            }
        }
    }
    let overlaps =
        |a: &std::ops::Range<usize>, b: &std::ops::Range<usize>| a.start < b.end && b.start < a.end;
    let opaque: Vec<_> = items
        .units
        .iter()
        .filter(|unit| unit.opaque)
        .flat_map(|unit| unit.ranges.iter())
        .collect();
    for (unit, _) in items.units.iter().zip(selected.iter()).filter(|(_, s)| **s) {
        if unit
            .ranges
            .iter()
            .any(|r| opaque.iter().any(|o| overlaps(r, o)))
        {
            return Some(Fallback::Opaque);
        }
    }
    let hidden = items
        .macro_calls
        .iter()
        .filter(|(called, _)| {
            !STD_MACROS.contains(&called.as_str()) || items.local_macros.contains(called)
        })
        .any(|(_, idents)| names.iter().any(|n| idents.contains(*n)));
    if hidden {
        return Some(Fallback::MacroName);
    }
    None
}

#[cfg(test)]
#[path = "tests/task_1790_mandate_traces_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/task_1800_trace_replay_tests.rs"]
mod replay_tests;
