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
}

#[derive(Debug, Default)]
pub(crate) struct Evidence {
    pub ids: BTreeSet<String>,
    /// Paths for which an item could not safely be attributed. Full-file
    /// union is the conservative fallback, never an empty evidence set.
    pub full_file_fallbacks: BTreeSet<String>,
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
        if !ordinary_source(path) {
            continue;
        }
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
        let diff = similar::TextDiff::from_lines(old, new);
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
            let (ids, fallback) = source_ids(path, text, &ranges, scope);
            out.ids.extend(ids);
            if fallback {
                out.full_file_fallbacks.insert(path.to_owned());
            }
        }
    }
    Ok(out)
}

/// Same source-language family as the existing trace scanner, without docs or
/// data/config formats. Test sources are included. Generated/build/store trees
/// are outside ordinary source scope. This is evidence expansion, not semantic
/// ownership discovery: untraced code and unlisted languages cannot identify IDs.
fn ordinary_source(path: &str) -> bool {
    if path.split('/').any(|p| {
        matches!(
            p,
            ".git"
                | ".aida"
                | ".aida-store"
                | "target"
                | "node_modules"
                | "dist"
                | "build"
                | ".cache"
                | ".venv"
                | "venv"
        )
    }) {
        return false;
    }
    matches!(
        Path::new(path).extension().and_then(|s| s.to_str()),
        Some(
            "rs" | "py"
                | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "go"
                | "java"
                | "c"
                | "cpp"
                | "h"
                | "hpp"
                | "cs"
                | "rb"
                | "sh"
        )
    )
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
    for run in text.split("trace:").skip(1) {
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
fn rust_comments(text: &str) -> Option<Vec<(usize, String)>> {
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

#[derive(Default)]
struct Items(Vec<std::ops::Range<usize>>);
impl<'ast> Visit<'ast> for Items {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.push(node.span());
        syn::visit::visit_item_fn(self, node);
    }
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        self.push(node.span());
        syn::visit::visit_item_impl(self, node);
    }
    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.push(node.span());
        syn::visit::visit_impl_item_fn(self, node);
    }
}
impl Items {
    fn push(&mut self, span: proc_macro2::Span) {
        self.0
            .push(span.start().line.saturating_sub(1)..span.end().line);
    }
}

fn source_ids(
    path: &str,
    text: &str,
    changes: &[std::ops::Range<usize>],
    scope: Scope,
) -> (BTreeSet<String>, bool) {
    if !path.ends_with(".rs") {
        // No proven item/comment parser for other languages: full-file raw
        // union is conservative (may include strings), never guessed attribution.
        return (trace_ids(text), matches!(scope, Scope::ChangedItems));
    }
    let Some(comments) = rust_comments(text) else {
        return (trace_ids(text), true);
    };
    let full = || comments.iter().flat_map(|(_, c)| trace_ids(c)).collect();
    if matches!(scope, Scope::FullFile) {
        return (full(), false);
    }
    // A mode-only change has no text hunks to attribute; it is still a
    // changed source file and therefore requires the signed full-file fallback.
    if changes.is_empty() {
        return (full(), true);
    }
    let Ok(file) = syn::parse_file(text) else {
        return (full(), true);
    };
    let mut items = Items::default();
    items.visit_file(&file);
    let lines: Vec<_> = text.lines().collect();
    // Plain leading comments are not part of syn's item span. Attach an
    // uninterrupted comment/blank prefix to its following function/impl.
    for range in &mut items.0 {
        while range.start > 0 {
            let i = range.start - 1;
            if lines[i].trim().is_empty()
                || (!comments[i].1.trim().is_empty() && lines[i].trim() == comments[i].1.trim())
            {
                range.start -= 1;
            } else {
                break;
            }
        }
    }
    // Select item ranges first, then parse each selected comment ONCE. A new
    // large file must not rescan an entire impl for every changed source line.
    let mut selected_items = BTreeSet::new();
    for change in changes {
        // Empty-side insertion/deletion boundaries are evidence too. Either
        // adjacent line must attribute to an item; otherwise use the full file.
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
            for item in items.0.iter().filter(|r| r.contains(&line)) {
                attributed = true;
                selected_items.insert((item.start, item.end));
            }
            if !attributed {
                return (full(), true);
            }
        }
    }
    let mut selected_lines = vec![false; comments.len()];
    for (start, end) in selected_items {
        selected_lines[start..end.min(comments.len())].fill(true);
    }
    let selected = comments
        .iter()
        .filter(|(line, _)| selected_lines[*line])
        .flat_map(|(_, comment)| trace_ids(comment))
        .collect();
    (selected, false)
}

#[cfg(test)]
#[path = "tests/task_1790_mandate_traces_tests.rs"]
mod tests;
