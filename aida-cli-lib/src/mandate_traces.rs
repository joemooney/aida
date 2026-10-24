//! Committed trace evidence for mandate EXCLUSIONS only. No completion or
//! author/reviewer authority may be derived from this module.
// trace:TASK-1790 | ai:codex
// mandate-gate

use anyhow::{ensure, Context, Result};
use std::{collections::BTreeSet, path::Path};
use syn::{spanned::Spanned, visit::Visit};

// Commands run at the store root. A plain path also works when the caller
// exports GIT_LITERAL_PATHSPECS=1, which disables Git's :(...) path magic.
// trace:BUG-2213 | ai:codex
const STORE_OBJECTS_PATH: &str = "objects";

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
    /// STORY-1700 (R1): every path the pinned diff changes (both names of a
    /// rename), for the mandate's gate-touching check.
    // trace:STORY-1700 | ai:claude
    pub paths: BTreeSet<String>,
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

/// Refuse moving or dirty exclusion evidence rather than mixing uncommitted
/// working objects with a historical exclusion read. Ref tips bind the existing
/// --all history reader, including sticky versions not reachable from store HEAD.
///
/// BUG-2195: the snapshot is the store HEAD plus every ref's tip (and, for
/// TASK-2144, every worktree's HEAD, which `--all` also walks), compared by
/// [`store_snapshot_stable`] rather than byte for byte, so an unrelated ref move
/// in the shared code repository no longer discards a collection attempt. A
/// dirty evidence tree is a [`StoreDirty`] error, which the caller may retry
/// (TASK-2019). Every snapshot git call is config-independent: no system
/// config, no replace objects, no path quoting, explicit status options.
// trace:TASK-2067 | ai:antigravity
pub(crate) fn store_snapshot(store: &Path) -> Result<StoreSnapshot> {
    // Only canonical object dirt can change this store's exclusion evidence.
    // Coordination leases/mail and other non-evidence files may be mid-write.
    // The home exclusion ledger is read separately by mandate_ship; its trust
    // checks do not depend on Git status here. Both queries run at the store
    // root and include replacement of the objects directory itself.
    // trace:BUG-2213 | ai:codex
    ensure!(
        snapshot_git(
            store,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--ignore-submodules=none",
                "--no-renames",
                "--",
                STORE_OBJECTS_PATH,
            ],
            None,
        )?
        .is_empty(),
        StoreDirty
    );
    // TASK-2238: status still honours $GIT_DIR/info/exclude and any
    // .gitignore, which could hide an untracked object. `ls-files --others`
    // with no exclude option applies none of them.
    // trace:TASK-2238 | ai:claude
    ensure!(
        snapshot_git(
            store,
            &["ls-files", "--others", "-z", "--", STORE_OBJECTS_PATH],
            None
        )?
        .is_empty(),
        StoreDirty
    );
    let head = snapshot_git(store, &["rev-parse", "--verify", "HEAD^{commit}"], None)?;
    let head = String::from_utf8(head).context("non-text store HEAD")?;
    let head = head.trim().to_owned();
    ensure!(oid(&head), "unreadable mandate store HEAD");
    let mut refs = std::collections::BTreeMap::new();
    let listing = snapshot_git(
        store,
        &["for-each-ref", "--format=%(objectname) %(refname)"],
        None,
    )?;
    for line in String::from_utf8(listing)
        .context("non-text store ref listing")?
        .lines()
    {
        let (tip, name) = line.split_once(' ').context("invalid store ref listing")?;
        ensure!(oid(tip), "invalid store ref tip for {name}");
        refs.insert(name.to_owned(), tip.to_owned());
    }
    // TASK-2144: `git log --all` also walks every worktree's HEAD, including a
    // detached one no ref names, so each is part of the snapshot. An unborn
    // HEAD (all zeros) holds no history.
    // trace:TASK-2067 | ai:claude
    let worktrees = snapshot_git(store, &["worktree", "list", "--porcelain", "-z"], None)?;
    let mut path = None;
    for field in String::from_utf8(worktrees)
        .context("non-text store worktree listing")?
        .split('\0')
    {
        if let Some(p) = field.strip_prefix("worktree ") {
            path = Some(p.to_owned());
        } else if let Some(tip) = field.strip_prefix("HEAD ") {
            let path = path.as_deref().context("invalid store worktree listing")?;
            ensure!(oid(tip), "invalid HEAD for store worktree {path}");
            if !tip.bytes().all(|b| b == b'0') {
                refs.insert(format!("HEAD of worktree {path}"), tip.to_owned());
            }
        }
    }
    Ok(StoreSnapshot { head, refs })
}

/// BUG-2195: what one [`store_snapshot`] observed.
// trace:TASK-2067 | ai:antigravity
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoreSnapshot {
    head: String,
    refs: std::collections::BTreeMap<String, String>,
}

/// TASK-2019: the store had uncommitted evidence when a snapshot was taken.
/// Retryable within the caller's bound; refusing after it.
// trace:TASK-2067 | ai:antigravity
#[derive(Debug)]
pub(crate) struct StoreDirty;

impl std::fmt::Display for StoreDirty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "mandate exclusion store has uncommitted evidence; retry after the store write completes",
        )
    }
}

impl std::error::Error for StoreDirty {}

/// BUG-2195: whether evidence gathered between `before` and `after` came from
/// one store version as far as the mandate exclusion reader can tell. Stable
/// iff HEAD is unchanged and no ref change can alter the `--all` history of
/// `objects/`:
/// - a changed or added ref must bring no commit touching `objects/` that no
///   earlier tip already reached;
/// - a ref moved to a tip that does not contain its old tip (force move) must
///   not drop any commit touching `objects/` (advisor condition C3);
/// - a deleted ref only removes versions, so the evidence already gathered
///   stays a superset and the attempt stays stable.
///
/// Any git failure is reported as unstable, so the caller re-collects and then
/// refuses after its bound (fail closed).
// trace:TASK-2067 | ai:antigravity
pub(crate) fn store_snapshot_stable(
    store: &Path,
    before: &StoreSnapshot,
    after: &StoreSnapshot,
) -> bool {
    if before.head != after.head {
        return false;
    }
    let mut new_tips: Vec<&str> = Vec::new();
    let mut forced: Vec<(&str, &str)> = Vec::new();
    for (name, tip) in &after.refs {
        match before.refs.get(name) {
            Some(old) if old == tip => {}
            Some(old) => {
                new_tips.push(tip);
                forced.push((old, tip));
            }
            None => new_tips.push(tip),
        }
    }
    if new_tips.is_empty() {
        return true;
    }
    // New objects/ history: commits reachable from a changed or added tip
    // that no earlier tip (or HEAD) reached.
    let mut input = String::new();
    for tip in &new_tips {
        input.push_str(tip);
        input.push('\n');
    }
    for tip in before
        .refs
        .values()
        .map(String::as_str)
        .chain([before.head.as_str()])
    {
        input.push('^');
        input.push_str(tip);
        input.push('\n');
    }
    if !no_objects_commit(store, &input) {
        return false;
    }
    // A changed ref that is not a fast-forward drops its old commits.
    for (old, new) in forced {
        let fast_forward = snapshot_status(store, &["merge-base", "--is-ancestor", old, new]);
        match fast_forward {
            Some(true) => {}
            Some(false) => {
                if !no_objects_commit(store, &format!("{old}\n^{new}\n")) {
                    return false;
                }
            }
            None => return false,
        }
    }
    true
}

/// `true` when the commit range on stdin (`rev-list --stdin` syntax) has no
/// commit touching `objects/`; `false` when it has one or git fails.
// trace:TASK-2067 | ai:antigravity
fn no_objects_commit(store: &Path, range: &str) -> bool {
    matches!(
        snapshot_git(
            store,
            &["rev-list", "--max-count=1", "--stdin", "--", "objects/"],
            Some(range.as_bytes()),
        ),
        Ok(out) if out.is_empty()
    )
}

/// Exit status of a snapshot git query: `Some(true)` for 0, `Some(false)` for
/// 1, `None` for anything else.
// trace:TASK-2067 | ai:antigravity
fn snapshot_status(store: &Path, args: &[&str]) -> Option<bool> {
    let status = snapshot_command(store).args(args).status().ok()?;
    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// The config-independent host git command every snapshot query uses.
// trace:TASK-2067 | ai:antigravity
fn snapshot_command(store: &Path) -> std::process::Command {
    let mut cmd = aida_core::git_ops::host_git_command(store);
    // TASK-2145: the user's global excludes must not hide an untracked object
    // from the dirty check. trace:TASK-2067 | ai:claude
    cmd.args([
        "-c",
        "core.quotePath=false",
        "-c",
        "core.excludesFile=/dev/null",
    ])
    .env("GIT_NO_REPLACE_OBJECTS", "1")
    .env("GIT_CONFIG_NOSYSTEM", "1")
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped());
    cmd
}

/// One snapshot git query; `Err` carries git's stderr.
// trace:TASK-2067 | ai:antigravity
fn snapshot_git(store: &Path, args: &[&str], stdin: Option<&[u8]>) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut cmd = snapshot_command(store);
    cmd.args(args);
    if stdin.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }
    let mut child = cmd
        .spawn()
        .context("reading the mandate exclusion store snapshot")?;
    let writer = stdin.map(|input| {
        let mut pipe = child.stdin.take().expect("piped stdin");
        let input = input.to_vec();
        std::thread::spawn(move || pipe.write_all(&input))
    });
    let out = child
        .wait_with_output()
        .context("reading the mandate exclusion store snapshot")?;
    if let Some(writer) = writer {
        writer
            .join()
            .map_err(|_| anyhow::anyhow!("snapshot stdin writer panicked"))?
            .context("writing to git")?;
    }
    ensure!(
        out.status.success(),
        "git {}: {}",
        args[0],
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(out.stdout)
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
    let raw = git(repo, &diff_tree_args(base, head))?;
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
            let (ids, fallback) = source_ids(path, text, &ranges, scope);
            out.ids.extend(ids);
            if fallback {
                out.full_file_fallbacks.insert(path.to_owned());
            }
        }
    }
    Ok(out)
}

/// The one `diff-tree` call [`collect`] reads its changed paths and blobs
/// from. Renames are off explicitly, so no rename config changes it.
// trace:TASK-1790 | ai:codex
// trace:TASK-2067 | ai:claude
fn diff_tree_args<'a>(base: &'a str, head: &'a str) -> [&'a str; 12] {
    [
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
    ]
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

#[derive(Default)]
struct Items {
    ranges: Vec<std::ops::Range<usize>>,
    module_opens: BTreeSet<(usize, usize)>,
}
impl<'ast> Visit<'ast> for Items {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if let Some((brace, _)) = &node.content {
            let start = brace.span.open().start();
            self.module_opens.insert((start.line, start.column));
        }
        syn::visit::visit_item_mod(self, node);
    }
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
        self.ranges
            .push(span.start().line.saturating_sub(1)..span.end().line);
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
    for range in &mut items.ranges {
        range.start = leading_comment_start(range.start, &lines, &comments);
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
            for item in items.ranges.iter().filter(|r| r.contains(&line)) {
                attributed = true;
                selected_items.insert((item.start, item.end));
            }
            if !attributed {
                return (full(), true);
            }
        }
    }
    let mut selected_lines = vec![false; comments.len()];
    // Module header comments apply to every item in the file, even when a use,
    // type or other sibling separates them from the changed function. Inner
    // doc comments belong to the file attributes, before its first item too.
    // This only adds evidence; item/fallback attribution remains unchanged.
    // trace:TASK-1800 | ai:codex
    let tokens = text.parse::<proc_macro2::TokenStream>().unwrap();
    let Some(header_end) = header_end(tokens.clone(), comments.len()) else {
        return (full(), true);
    };
    selected_lines[..header_end.min(comments.len())].fill(true);
    // Comments not owned by any function/impl (including its leading prefix)
    // are module evidence, wherever they occur in the file. This is additive:
    // sibling item traces remain scoped and attribution fallback is unchanged.
    // trace:TASK-1800 | ai:codex
    let mut owned_lines = vec![false; comments.len()];
    for range in &items.ranges {
        owned_lines[range.start..range.end.min(comments.len())].fill(true);
    }
    for (line, owned) in owned_lines.into_iter().enumerate() {
        selected_lines[line] |= !owned;
    }
    // A module header may be attached as a leading comment to its first fn;
    // it still applies independently of which function changed. Use the actual
    // syn module brace identities, never arbitrary function/macro brace groups.
    if select_module_headers(tokens, &items.module_opens, &mut selected_lines).is_none() {
        return (full(), true);
    }
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

#[cfg(test)]
#[path = "tests/task_1800_trace_replay_tests.rs"]
mod replay_tests;

#[cfg(test)]
#[path = "tests/bug_2195_store_snapshot_tests.rs"]
mod snapshot_tests;
