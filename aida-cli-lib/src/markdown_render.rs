//! Render markdown for a human reading it in a terminal.
//!
//! `aida show` used to print descriptions and comments as raw markdown:
//! literal `**bold**`, `## headings`, pipe tables. This module turns that
//! source into terminal text: styled headings and emphasis, a distinct style
//! for inline code, gutter-indented code blocks, hanging-indent lists, and
//! box-drawn tables wrapped to the terminal width.
//!
//! The renderer is a small walk over `pulldown-cmark` events (already in the
//! workspace) rather than a terminal-markdown crate, so it adds no new
//! dependencies and parses the same CommonMark dialect the HTML reports use.
//!
//! Contract:
//! - [`render_markdown_for_terminal`] never fails. Any renderer error (or
//!   panic) yields the source text unchanged.
//! - Styling is plain ANSI SGR and is dropped when colour is off (`NO_COLOR`,
//!   a forced no-colour override); the layout (wrapping, list indentation,
//!   table alignment) is kept.
//! - Rules, gutters, bullets and table borders follow the glyph profile
//!   (`AIDA_GLYPHS` / `[ui] glyphs`): the ascii profile draws them with
//!   `= - * | +` only.
//! - Callers decide whether to render at all with [`show_renders_markdown`]:
//!   only human output at an interactive terminal, and never under `--raw` /
//!   `AIDA_MARKDOWN=raw`. Machine formats and piped output keep the source.
// trace:STORY-1652 | ai:claude

use crate::glyphs::{Glyph, GlyphProfile, LineArt};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Environment variable that forces raw markdown in human output when set to
/// `raw` (the env spelling of `aida show --raw`).
pub(crate) const MARKDOWN_ENV: &str = "AIDA_MARKDOWN";

/// Narrowest width the renderer lays out for. Below this, wrapping and table
/// fitting stop being useful, so the layout simply overflows.
const MIN_WIDTH: usize = 20;

/// Narrowest body a list item is wrapped to, however deep the nesting.
const MIN_LIST_BODY: usize = 12;

/// Why rendering was refused. The caller prints the raw source instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MarkdownRenderError {
    /// A fenced code block is opened and never closed. CommonMark would turn
    /// everything after it into code, which hides the author's typo behind a
    /// confusing rendering; the raw text is clearer.
    UnclosedCodeFence,
    /// The parser's event stream did not nest the way the renderer expected.
    Structure(&'static str),
}

impl std::fmt::Display for MarkdownRenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnclosedCodeFence => write!(f, "unclosed code fence"),
            Self::Structure(what) => write!(f, "unexpected markdown structure: {what}"),
        }
    }
}

/// Decide whether `aida show`'s human output renders markdown.
///
/// Pure so it is unit-tested: `raw_flag` is `--raw`, `env` is the value of
/// `AIDA_MARKDOWN`, `stdout_is_tty` whether stdout is an interactive terminal.
/// The caller only asks on the human path, so machine formats never reach it.
pub(crate) fn markdown_render_decision(
    raw_flag: bool,
    env: Option<&str>,
    stdout_is_tty: bool,
) -> bool {
    if raw_flag || !stdout_is_tty {
        return false;
    }
    !matches!(env.map(|v| v.trim().to_ascii_lowercase()), Some(v) if v == "raw")
}

/// [`markdown_render_decision`] with the ambient environment and terminal.
pub(crate) fn show_renders_markdown(raw_flag: bool) -> bool {
    let env = std::env::var(MARKDOWN_ENV).ok();
    markdown_render_decision(
        raw_flag,
        env.as_deref(),
        crate::authority_stdout_is_terminal(),
    )
}

/// Width to lay out for: the terminal's column count, else `COLUMNS`, else 80.
pub(crate) fn terminal_width() -> usize {
    if let Ok((cols, _)) = crossterm::terminal::size() {
        if cols > 0 {
            return cols as usize;
        }
    }
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.trim().parse::<usize>().ok())
        .filter(|c| *c > 0)
        .unwrap_or(80)
}

/// The process-wide colour override that applies no-color.org's rule to an
/// EMPTY `NO_COLOR`, or `None` when `colored`'s own reading is already right.
///
/// no-color.org says an empty `NO_COLOR` must be ignored; `colored` 2.2 treats
/// any set value (empty included) as "off". So only for an empty `NO_COLOR`
/// with no `CLICOLOR_FORCE` forcing colour on, this returns the decision
/// `colored` would make without `NO_COLOR`: on at a terminal unless
/// `CLICOLOR=0`. Pure so it is unit-tested.
// trace:BUG-1979 | ai:claude
pub(crate) fn empty_no_color_override(
    no_color: Option<&std::ffi::OsStr>,
    clicolor_force: Option<&str>,
    clicolor: Option<&str>,
    stdout_is_tty: bool,
) -> Option<bool> {
    if !no_color.is_some_and(|v| v.is_empty()) {
        return None;
    }
    if clicolor_force.is_some_and(|v| v != "0") {
        return None;
    }
    Some(clicolor != Some("0") && stdout_is_tty)
}

/// Apply [`empty_no_color_override`] to the `colored` crate once, at process
/// start (called from `main_entry` before any thread starts), so the whole
/// process — the markdown renderer and every `colored` call site — makes the
/// same colour decision. A later explicit `--color` / `--no-color` override
/// still wins because it is applied after this.
// trace:BUG-1979 | ai:claude
pub(crate) fn apply_no_color_policy() {
    use std::io::IsTerminal;
    apply_no_color_policy_to(
        &colored::control::SHOULD_COLORIZE,
        std::io::stdout().is_terminal(),
    );
}

/// [`apply_no_color_policy`] against an explicit `colored` decision object, so
/// tests can use a local one instead of flipping the process-wide global.
fn apply_no_color_policy_to(target: &colored::control::ShouldColorize, stdout_is_tty: bool) {
    let var = |k: &str| std::env::var(k).ok();
    if let Some(on) = empty_no_color_override(
        std::env::var_os("NO_COLOR").as_deref(),
        var("CLICOLOR_FORCE").as_deref(),
        var("CLICOLOR").as_deref(),
        stdout_is_tty,
    ) {
        target.set_override(on);
    }
}

thread_local! {
    /// Set while this thread renders inside [`catch_render`], so the panic
    /// hook stays silent for that panic only.
    static RENDERING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Wrap `previous` in a panic hook that is silent for a panic raised while
/// this thread is inside [`catch_render`], and delegates every other panic
/// (other threads, or this thread outside a render) to `previous`.
// trace:BUG-1979 | ai:claude
fn quiet_render_hook(
    previous: Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>,
) -> Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static> {
    Box::new(move |info| {
        if !RENDERING.with(|r| r.get()) {
            previous(info);
        }
    })
}

/// Install [`quiet_render_hook`] over the current hook, once per process. A
/// caught renderer panic falls back to the raw source, so the default hook's
/// stderr message would only be noise.
// trace:BUG-1979 | ai:claude
fn install_quiet_render_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(quiet_render_hook(previous));
    });
}

/// `catch_unwind(f)` with this thread flagged as rendering, so a panic in `f`
/// is caught without a message from the quiet hook.
// trace:BUG-1979 | ai:claude
fn catch_render<T>(f: impl FnOnce() -> T + std::panic::UnwindSafe) -> std::thread::Result<T> {
    install_quiet_render_hook();
    let was = RENDERING.with(|r| r.replace(true));
    let result = std::panic::catch_unwind(f);
    RENDERING.with(|r| r.set(was));
    result
}

/// The message carried by a panic payload, for the debug diagnostic.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic payload")
}

/// Render `src` for a terminal `width` columns wide. Never fails: on any
/// renderer error or panic the source is returned unchanged, without a panic
/// message on stderr (set `AIDA_DEBUG` to see it). Glyphs follow the active
/// glyph profile; a configured `[ui] theme` contributes its base profile.
pub(crate) fn render_markdown_for_terminal(src: &str, width: usize) -> String {
    render_for_terminal_with(src, width, &colored::control::SHOULD_COLORIZE)
}

/// [`render_markdown_for_terminal`] with styling decided by `colour`, the
/// `colored` crate's decision object. Production passes the global, so the
/// renderer honours `NO_COLOR`, `CLICOLOR_FORCE` and any in-process override
/// (including [`apply_no_color_policy`]) exactly like the rest of the output.
// trace:BUG-1979 | ai:claude
fn render_for_terminal_with(
    src: &str,
    width: usize,
    colour: &colored::control::ShouldColorize,
) -> String {
    let styled = colour.should_colorize();
    // trace:BUG-1979 | ai:claude
    let root = crate::find_project_root().ok();
    // A valid `AIDA_GLYPHS` (a per-terminal property) beats a configured
    // theme's base; an unparseable value is ignored, as `active_profile` does.
    let env_profile = std::env::var("AIDA_GLYPHS")
        .ok()
        .and_then(|v| GlyphProfile::parse(&v));
    let profile = match (env_profile, crate::glyphs::active_theme(root.as_deref())) {
        (None, Some(theme)) => theme.base,
        _ => crate::glyphs::active_profile(root.as_deref()),
    };
    match catch_render(|| render_markdown_with(src, width, styled, profile)) {
        Ok(Ok(rendered)) => rendered,
        Ok(Err(_)) => src.to_string(),
        Err(payload) => {
            if std::env::var_os("AIDA_DEBUG").is_some() {
                eprintln!(
                    "aida: markdown renderer panicked, showing raw text: {}",
                    panic_message(payload.as_ref())
                );
            }
            src.to_string()
        }
    }
}

/// Render `src` to terminal text with the default (Unicode) glyph profile.
/// See [`render_markdown_with`].
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn render_markdown(
    src: &str,
    width: usize,
    styled: bool,
) -> Result<String, MarkdownRenderError> {
    render_markdown_with(src, width, styled, GlyphProfile::Unicode)
}

/// Render `src` to terminal text, with ANSI styling when `styled` and rules,
/// gutters, bullets and table borders drawn in `profile`'s glyphs. Lines are
/// joined with `\n`, no trailing newline.
// trace:BUG-1979 | ai:claude
pub(crate) fn render_markdown_with(
    src: &str,
    width: usize,
    styled: bool,
    profile: GlyphProfile,
) -> Result<String, MarkdownRenderError> {
    let blocks = parse_blocks(src)?;
    let width = width.max(MIN_WIDTH);
    let ctx = Ctx {
        styled,
        art: LineArt::for_profile(profile),
        bullet: Glyph::Bullet.render(profile),
    };
    let lines = ctx.render_blocks(&blocks, width, true);
    Ok(lines
        .into_iter()
        .map(|l| l.text.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n"))
}

// ---------------------------------------------------------------------------
// Inline model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Style {
    bold: bool,
    italic: bool,
    code: bool,
    strike: bool,
    dim: bool,
    underline: bool,
}

impl Style {
    fn is_plain(&self) -> bool {
        *self == Style::default()
    }
}

#[derive(Debug, Clone)]
enum Inline {
    Text(String, Style),
    /// A hard line break.
    Break,
}

type Inlines = Vec<Inline>;

// ---------------------------------------------------------------------------
// Block model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Block {
    Heading(u8, Inlines),
    Para(Inlines),
    Code(String),
    /// A raw HTML block, shown as written in a code gutter.
    Html(String),
    Quote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Table {
        aligns: Vec<Alignment>,
        head: Vec<Inlines>,
        rows: Vec<Vec<Inlines>>,
    },
    Rule,
}

#[derive(Debug, Clone, Default)]
struct Item {
    task: Option<bool>,
    blocks: Vec<Block>,
}

/// Builder frames, one per open container.
enum Frame {
    Root(Vec<Block>),
    Quote(Vec<Block>),
    Footnote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    /// A list item; `pending` collects tight-list inline text that arrives
    /// without a paragraph wrapper.
    Item {
        task: Option<bool>,
        blocks: Vec<Block>,
        pending: Inlines,
    },
    Para(Inlines),
    Heading(u8, Inlines),
    Code(String),
    Table {
        aligns: Vec<Alignment>,
        head: Vec<Inlines>,
        rows: Vec<Vec<Inlines>>,
        row: Vec<Inlines>,
    },
    Cell(Inlines),
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// True when a fenced block's source span ends without its closing fence.
/// `quote_depth` is how many blockquote `>` markers precede the opening
/// fence on its line. The span starts at the fence itself, so its later lines
/// carry exactly that many markers; only those are stripped, so a `> ```` line
/// inside an unquoted fence stays content and does not close it.
fn fence_is_unclosed(span: &str, quote_depth: usize) -> bool {
    let lines: Vec<&str> = span
        .trim_end_matches(['\n', '\r'])
        .lines()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                l
            } else {
                strip_quote_markers(l, quote_depth)
            }
        })
        .collect();
    let Some(first) = lines.first() else {
        return true;
    };
    let open = first.trim_start();
    let fence_char = match open.chars().next() {
        Some(c @ ('`' | '~')) => c,
        _ => return false,
    };
    let open_len = open.chars().take_while(|c| *c == fence_char).count();
    if lines.len() < 2 {
        return true;
    }
    let last = lines[lines.len() - 1].trim();
    !(last.chars().all(|c| c == fence_char) && last.chars().count() >= open_len)
}

/// Strip up to `depth` leading blockquote `>` markers (and the whitespace
/// around them) from a source line.
fn strip_quote_markers(line: &str, depth: usize) -> &str {
    let mut rest = line;
    for _ in 0..depth {
        match rest.trim_start().strip_prefix('>') {
            Some(after) => rest = after,
            None => break,
        }
    }
    rest
}

/// Blockquote depth of the source line holding `offset`: the `>` markers
/// between the start of that line and `offset`.
fn quote_depth_at(src: &str, offset: usize) -> usize {
    let line_start = src[..offset].rfind('\n').map_or(0, |i| i + 1);
    src[line_start..offset].matches('>').count()
}

/// Whether two raw-HTML events belong to one block: the source between
/// them holds only blockquote prefixes (`>`, spaces, tabs). Each event's
/// range already ends after its newline, so any newline in the gap is a
/// blank line, which separates blocks.
fn html_gap_is_contiguous(gap: &str) -> bool {
    gap.chars().all(|c| matches!(c, ' ' | '\t' | '>'))
}

struct Builder {
    stack: Vec<Frame>,
    style: Style,
    /// Destination URLs of open links, so the URL can follow the link text.
    links: Vec<(String, usize)>,
    /// Nesting depth of each span style, so `*a *b* c*`-style nesting only
    /// clears the style when the outermost span closes.
    italic_depth: usize,
    bold_depth: usize,
    strike_depth: usize,
    /// Source offset where the last raw-HTML event ended, so consecutive
    /// lines of one HTML block merge into a single block.
    last_html_end: Option<usize>,
}

impl Builder {
    fn push_inline(&mut self, inline: Inline) -> Result<(), MarkdownRenderError> {
        match self.stack.last_mut() {
            Some(Frame::Para(v)) | Some(Frame::Heading(_, v)) | Some(Frame::Cell(v)) => {
                v.push(inline);
            }
            Some(Frame::Item { pending, .. }) => pending.push(inline),
            Some(Frame::Code(buf)) => {
                if let Inline::Text(t, _) = inline {
                    buf.push_str(&t);
                }
            }
            // Inline content directly inside a block container (e.g. raw HTML
            // at the top level): give it its own paragraph.
            Some(Frame::Root(_)) | Some(Frame::Quote(_)) | Some(Frame::Footnote(_)) => {
                self.push_block(Block::Para(vec![inline]))?;
            }
            _ => return Err(MarkdownRenderError::Structure("inline outside a block")),
        }
        Ok(())
    }

    fn inline_len(&self) -> usize {
        match self.stack.last() {
            Some(Frame::Para(v)) | Some(Frame::Heading(_, v)) | Some(Frame::Cell(v)) => v.len(),
            Some(Frame::Item { pending, .. }) => pending.len(),
            _ => 0,
        }
    }

    fn inlines_since(&self, from: usize) -> String {
        let v = match self.stack.last() {
            Some(Frame::Para(v)) | Some(Frame::Heading(_, v)) | Some(Frame::Cell(v)) => v,
            Some(Frame::Item { pending, .. }) => pending,
            _ => return String::new(),
        };
        v.iter()
            .skip(from)
            .filter_map(|i| match i {
                Inline::Text(t, _) => Some(t.as_str()),
                Inline::Break => None,
            })
            .collect()
    }

    fn push_block(&mut self, block: Block) -> Result<(), MarkdownRenderError> {
        match self.stack.last_mut() {
            Some(Frame::Root(v)) | Some(Frame::Quote(v)) | Some(Frame::Footnote(v)) => {
                v.push(block);
                Ok(())
            }
            Some(Frame::Item {
                blocks, pending, ..
            }) => {
                if !pending.is_empty() {
                    blocks.push(Block::Para(std::mem::take(pending)));
                }
                blocks.push(block);
                Ok(())
            }
            _ => Err(MarkdownRenderError::Structure("block outside a container")),
        }
    }

    fn container_accepts_html(&self) -> bool {
        matches!(
            self.stack.last(),
            Some(Frame::Root(_)) | Some(Frame::Quote(_)) | Some(Frame::Footnote(_))
        )
    }

    /// Append to the HTML block that ends the current container. False when
    /// the container's last block is not HTML.
    fn append_html(&mut self, text: &str) -> bool {
        let blocks = match self.stack.last_mut() {
            Some(Frame::Root(v)) | Some(Frame::Quote(v)) | Some(Frame::Footnote(v)) => v,
            _ => return false,
        };
        match blocks.last_mut() {
            Some(Block::Html(buf)) => {
                buf.push_str(text);
                true
            }
            _ => false,
        }
    }

    fn pop(&mut self) -> Result<Frame, MarkdownRenderError> {
        self.stack
            .pop()
            .ok_or(MarkdownRenderError::Structure("unbalanced end tag"))
    }

    fn text(&mut self, text: &str) -> Result<(), MarkdownRenderError> {
        let style = self.style;
        self.push_inline(Inline::Text(text.to_string(), style))
    }

    fn start(&mut self, tag: Tag<'_>) -> Result<(), MarkdownRenderError> {
        match tag {
            Tag::Paragraph => self.stack.push(Frame::Para(Vec::new())),
            Tag::Heading(level, _, _) => self
                .stack
                .push(Frame::Heading(heading_level(level), Vec::new())),
            Tag::BlockQuote => self.stack.push(Frame::Quote(Vec::new())),
            Tag::CodeBlock(_) => self.stack.push(Frame::Code(String::new())),
            Tag::List(start) => self.stack.push(Frame::List {
                start,
                items: Vec::new(),
            }),
            Tag::Item => self.stack.push(Frame::Item {
                task: None,
                blocks: Vec::new(),
                pending: Vec::new(),
            }),
            Tag::FootnoteDefinition(label) => {
                self.stack
                    .push(Frame::Footnote(vec![Block::Para(vec![Inline::Text(
                        format!("[^{label}]:"),
                        Style::default(),
                    )])]));
            }
            Tag::Table(aligns) => self.stack.push(Frame::Table {
                aligns,
                head: Vec::new(),
                rows: Vec::new(),
                row: Vec::new(),
            }),
            Tag::TableHead | Tag::TableRow => {}
            Tag::TableCell => self.stack.push(Frame::Cell(Vec::new())),
            Tag::Emphasis => {
                self.italic_depth += 1;
                self.style.italic = true;
            }
            Tag::Strong => {
                self.bold_depth += 1;
                self.style.bold = true;
            }
            Tag::Strikethrough => {
                self.strike_depth += 1;
                self.style.strike = true;
            }
            Tag::Link(_, url, _) => {
                let at = self.inline_len();
                self.links.push((url.to_string(), at));
                self.style.underline = true;
            }
            Tag::Image(_, url, _) => {
                let at = self.inline_len();
                self.links.push((url.to_string(), at));
            }
        }
        Ok(())
    }

    fn end(&mut self, tag: Tag<'_>) -> Result<(), MarkdownRenderError> {
        match tag {
            Tag::Paragraph => match self.pop()? {
                Frame::Para(v) => self.push_block(Block::Para(v)),
                _ => Err(MarkdownRenderError::Structure("paragraph end")),
            },
            Tag::Heading(..) => match self.pop()? {
                Frame::Heading(level, v) => self.push_block(Block::Heading(level, v)),
                _ => Err(MarkdownRenderError::Structure("heading end")),
            },
            Tag::BlockQuote => match self.pop()? {
                Frame::Quote(v) => self.push_block(Block::Quote(v)),
                _ => Err(MarkdownRenderError::Structure("quote end")),
            },
            Tag::CodeBlock(_) => match self.pop()? {
                Frame::Code(buf) => self.push_block(Block::Code(buf)),
                _ => Err(MarkdownRenderError::Structure("code end")),
            },
            Tag::List(_) => match self.pop()? {
                Frame::List { start, items } => self.push_block(Block::List { start, items }),
                _ => Err(MarkdownRenderError::Structure("list end")),
            },
            Tag::Item => match self.pop()? {
                Frame::Item {
                    task,
                    mut blocks,
                    pending,
                } => {
                    if !pending.is_empty() {
                        blocks.push(Block::Para(pending));
                    }
                    match self.stack.last_mut() {
                        Some(Frame::List { items, .. }) => {
                            items.push(Item { task, blocks });
                            Ok(())
                        }
                        _ => Err(MarkdownRenderError::Structure("item outside a list")),
                    }
                }
                _ => Err(MarkdownRenderError::Structure("item end")),
            },
            Tag::FootnoteDefinition(_) => {
                match self.pop()? {
                    Frame::Footnote(mut v) => {
                        // Join "[^label]:" with the first body paragraph.
                        if v.len() >= 2 {
                            if let (Block::Para(label), Block::Para(_)) = (&v[0], &v[1]) {
                                let mut label = label.clone();
                                if let Block::Para(body) = v.remove(1) {
                                    label.push(Inline::Text(" ".into(), Style::default()));
                                    label.extend(body);
                                }
                                v[0] = Block::Para(label);
                            }
                        }
                        for b in v {
                            self.push_block(b)?;
                        }
                        Ok(())
                    }
                    _ => Err(MarkdownRenderError::Structure("footnote end")),
                }
            }
            Tag::Table(_) => match self.pop()? {
                Frame::Table {
                    aligns, head, rows, ..
                } => self.push_block(Block::Table { aligns, head, rows }),
                _ => Err(MarkdownRenderError::Structure("table end")),
            },
            Tag::TableHead => match self.stack.last_mut() {
                Some(Frame::Table { head, row, .. }) => {
                    *head = std::mem::take(row);
                    Ok(())
                }
                _ => Err(MarkdownRenderError::Structure("table head end")),
            },
            Tag::TableRow => match self.stack.last_mut() {
                Some(Frame::Table { rows, row, .. }) => {
                    rows.push(std::mem::take(row));
                    Ok(())
                }
                _ => Err(MarkdownRenderError::Structure("table row end")),
            },
            Tag::TableCell => match self.pop()? {
                Frame::Cell(v) => match self.stack.last_mut() {
                    Some(Frame::Table { row, .. }) => {
                        row.push(v);
                        Ok(())
                    }
                    _ => Err(MarkdownRenderError::Structure("cell outside a table")),
                },
                _ => Err(MarkdownRenderError::Structure("cell end")),
            },
            Tag::Emphasis => {
                self.italic_depth = self.italic_depth.saturating_sub(1);
                self.style.italic = self.italic_depth > 0;
                Ok(())
            }
            Tag::Strong => {
                self.bold_depth = self.bold_depth.saturating_sub(1);
                self.style.bold = self.bold_depth > 0;
                Ok(())
            }
            Tag::Strikethrough => {
                self.strike_depth = self.strike_depth.saturating_sub(1);
                self.style.strike = self.strike_depth > 0;
                Ok(())
            }
            Tag::Link(..) | Tag::Image(..) => {
                let is_link = matches!(tag, Tag::Link(..));
                if is_link {
                    self.style.underline = false;
                }
                let (url, at) = self
                    .links
                    .pop()
                    .ok_or(MarkdownRenderError::Structure("link end"))?;
                let label = self.inlines_since(at);
                if !is_link && label.is_empty() {
                    self.text("[image]")?;
                }
                let url_shown = url.trim_start_matches("mailto:");
                if !url.is_empty() && label.trim() != url_shown && label.trim() != url {
                    let style = Style {
                        dim: true,
                        ..Style::default()
                    };
                    self.push_inline(Inline::Text(format!(" ({url})"), style))?;
                }
                Ok(())
            }
        }
    }
}

fn parse_blocks(src: &str) -> Result<Vec<Block>, MarkdownRenderError> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let mut b = Builder {
        stack: vec![Frame::Root(Vec::new())],
        style: Style::default(),
        links: Vec::new(),
        italic_depth: 0,
        bold_depth: 0,
        strike_depth: 0,
        last_html_end: None,
    };
    for (event, range) in Parser::new_ext(src, options).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if let Tag::CodeBlock(CodeBlockKind::Fenced(_)) = &tag {
                    if fence_is_unclosed(&src[range.clone()], quote_depth_at(src, range.start)) {
                        return Err(MarkdownRenderError::UnclosedCodeFence);
                    }
                }
                b.start(tag)?;
            }
            Event::End(tag) => b.end(tag)?,
            Event::Text(t) => b.text(&t)?,
            Event::Code(t) => {
                let style = Style {
                    code: true,
                    ..b.style
                };
                b.push_inline(Inline::Text(t.to_string(), style))?;
            }
            Event::Html(t) => {
                // Raw HTML is shown as written. pulldown-cmark emits an HTML
                // block one line per event; events that abut in the source
                // are one block and are merged into a single gutter block.
                let contiguous = b.last_html_end.is_some_and(|end| {
                    end <= range.start && html_gap_is_contiguous(&src[end..range.start])
                });
                b.last_html_end = Some(range.end);
                if b.container_accepts_html() {
                    if !(contiguous && b.append_html(&t)) {
                        b.push_block(Block::Html(t.to_string()))?;
                    }
                } else {
                    b.text(t.trim_end_matches('\n'))?;
                }
            }
            Event::FootnoteReference(label) => b.text(&format!("[^{label}]"))?,
            Event::SoftBreak => b.text(" ")?,
            Event::HardBreak => b.push_inline(Inline::Break)?,
            Event::Rule => b.push_block(Block::Rule)?,
            Event::TaskListMarker(done) => {
                if let Some(Frame::Item { task, .. }) = b.stack.last_mut() {
                    *task = Some(done);
                }
            }
        }
    }
    match (b.stack.pop(), b.stack.is_empty()) {
        (Some(Frame::Root(blocks)), true) => Ok(blocks),
        _ => Err(MarkdownRenderError::Structure("unclosed container")),
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

struct Ctx {
    styled: bool,
    /// Rules, gutters and table borders for the active glyph profile.
    // trace:BUG-1979 | ai:claude
    art: LineArt,
    /// Unordered-list marker for the active glyph profile.
    bullet: &'static str,
}

/// A rendered line plus its visible (display) width, which excludes ANSI.
#[derive(Debug, Clone)]
struct Line {
    text: String,
    width: usize,
}

impl Line {
    fn empty() -> Self {
        Line {
            text: String::new(),
            width: 0,
        }
    }
}

/// A run of non-space text made of differently styled pieces.
#[derive(Debug, Clone, Default)]
struct Word {
    pieces: Vec<(String, Style)>,
    width: usize,
}

impl Ctx {
    fn paint(&self, text: &str, style: Style) -> String {
        if !self.styled || style.is_plain() || text.is_empty() {
            return text.to_string();
        }
        let mut codes: Vec<&str> = Vec::new();
        if style.bold {
            codes.push("1");
        }
        if style.dim {
            codes.push("2");
        }
        if style.italic {
            codes.push("3");
        }
        if style.underline {
            codes.push("4");
        }
        if style.strike {
            codes.push("9");
        }
        if style.code {
            codes.push("36");
        }
        format!("\x1b[{}m{}\x1b[0m", codes.join(";"), text)
    }

    fn dim(&self, text: &str) -> String {
        self.paint(
            text,
            Style {
                dim: true,
                ..Style::default()
            },
        )
    }

    /// Wrap inline content to `width` columns, applying `extra` style on top
    /// of each piece's own style. Never returns an empty vector.
    fn wrap(&self, inlines: &[Inline], width: usize, extra: Style) -> Vec<Line> {
        let width = width.max(1);
        // Paragraphs separated by hard breaks are wrapped independently.
        let mut segments: Vec<Vec<Word>> = vec![Vec::new()];
        let mut word = Word::default();
        let flush = |word: &mut Word, segs: &mut Vec<Vec<Word>>| {
            if !word.pieces.is_empty() {
                segs.last_mut().unwrap().push(std::mem::take(word));
            }
        };
        for inline in inlines {
            match inline {
                Inline::Break => {
                    flush(&mut word, &mut segments);
                    segments.push(Vec::new());
                }
                Inline::Text(text, style) => {
                    // Without styling, inline code keeps its backticks so it
                    // still reads as code.
                    let quoted;
                    let text = if style.code && !self.styled {
                        quoted = format!("`{text}`");
                        &quoted
                    } else {
                        text
                    };
                    let style = merge(*style, extra);
                    let mut piece = String::new();
                    for ch in text.chars() {
                        if ch == ' ' || ch == '\t' || ch == '\n' {
                            if !piece.is_empty() {
                                word.width += piece.width();
                                word.pieces.push((std::mem::take(&mut piece), style));
                            }
                            flush(&mut word, &mut segments);
                        } else {
                            piece.push(ch);
                        }
                    }
                    if !piece.is_empty() {
                        word.width += piece.width();
                        word.pieces.push((piece, style));
                    }
                }
            }
        }
        flush(&mut word, &mut segments);

        let mut lines = Vec::new();
        for words in segments {
            let mut cur = Line::empty();
            for w in words {
                for w in split_word(w, width) {
                    let need = if cur.width == 0 { w.width } else { w.width + 1 };
                    if cur.width > 0 && cur.width + need > width {
                        lines.push(std::mem::replace(&mut cur, Line::empty()));
                    }
                    if cur.width > 0 {
                        cur.text.push(' ');
                        cur.width += 1;
                    }
                    for (text, style) in &w.pieces {
                        cur.text.push_str(&self.paint(text, *style));
                    }
                    cur.width += w.width;
                }
            }
            lines.push(cur);
        }
        lines
    }

    fn render_blocks(&self, blocks: &[Block], width: usize, loose: bool) -> Vec<Line> {
        let mut out: Vec<Line> = Vec::new();
        let mut prev: Option<&Block> = None;
        for block in blocks {
            if let Some(p) = prev {
                // At the top level every block is separated by a blank line.
                // Inside a list item only consecutive paragraphs are.
                let gap = loose || matches!((p, block), (Block::Para(_), Block::Para(_)));
                if gap {
                    out.push(Line::empty());
                }
            }
            out.extend(self.render_block(block, width));
            prev = Some(block);
        }
        out
    }

    fn render_block(&self, block: &Block, width: usize) -> Vec<Line> {
        match block {
            Block::Para(inlines) => self.wrap(inlines, width, Style::default()),
            Block::Heading(level, inlines) => {
                let style = Style {
                    bold: true,
                    underline: *level == 1,
                    italic: *level >= 4,
                    ..Style::default()
                };
                let mut lines = self.wrap(inlines, width, style);
                if *level <= 2 {
                    let w = lines.iter().map(|l| l.width).max().unwrap_or(0).max(1);
                    // trace:BUG-1979 | ai:claude
                    let ch = if *level == 1 {
                        self.art.rule_heavy
                    } else {
                        self.art.rule_light
                    };
                    lines.push(Line {
                        text: self.dim(&ch.repeat(w)),
                        width: w,
                    });
                }
                lines
            }
            Block::Code(code) | Block::Html(code) => {
                let gutter = self.dim(self.art.vertical); // trace:BUG-1979 | ai:claude
                let body = code.strip_suffix('\n').unwrap_or(code);
                body.split('\n')
                    .map(|l| {
                        let l = l.trim_end_matches('\r');
                        Line {
                            text: format!("  {gutter} {l}"),
                            width: 4 + l.width(),
                        }
                    })
                    .collect()
            }
            Block::Quote(blocks) => {
                let inner = self.render_blocks(blocks, width.saturating_sub(2).max(1), true);
                let bar = self.dim(self.art.vertical); // trace:BUG-1979 | ai:claude
                inner
                    .into_iter()
                    .map(|l| Line {
                        text: format!("{bar} {}", l.text),
                        width: l.width + 2,
                    })
                    .collect()
            }
            Block::List { start, items } => self.render_list(*start, items, width),
            Block::Table { aligns, head, rows } => self.render_table(aligns, head, rows, width),
            Block::Rule => {
                let w = width.min(40);
                vec![Line {
                    text: self.dim(&self.art.rule_light.repeat(w)), // trace:BUG-1979 | ai:claude
                    width: w,
                }]
            }
        }
    }

    fn render_list(&self, start: Option<u64>, items: &[Item], width: usize) -> Vec<Line> {
        let mut out = Vec::new();
        let last_number = start.map(|s| s + items.len().saturating_sub(1) as u64);
        let marker_width = match last_number {
            Some(n) => n.to_string().len() + 2,
            None => 2,
        };
        for (i, item) in items.iter().enumerate() {
            let mut marker = match start {
                Some(s) => format!("{}.", s + i as u64),
                None => self.bullet.to_string(), // trace:BUG-1979 | ai:claude
            };
            // Deeply nested lists keep a readable floor and overflow the
            // terminal rather than wrapping a word per line.
            let mut body_width = width.saturating_sub(marker_width).max(MIN_LIST_BODY);
            let mut task_prefix = String::new();
            if let Some(done) = item.task {
                task_prefix = if done { "[x] " } else { "[ ] " }.to_string();
                body_width = body_width.saturating_sub(4).max(MIN_LIST_BODY);
            }
            let body = self.render_blocks(&item.blocks, body_width, false);
            let indent = " ".repeat(marker_width + task_prefix.width());
            let pad = marker_width.saturating_sub(marker.width());
            marker.push_str(&" ".repeat(pad));
            if body.is_empty() {
                out.push(Line {
                    width: marker.width() + task_prefix.width(),
                    text: format!("{}{}", self.dim(&marker), task_prefix),
                });
                continue;
            }
            for (j, line) in body.into_iter().enumerate() {
                if j == 0 {
                    out.push(Line {
                        width: marker.width() + task_prefix.width() + line.width,
                        text: format!("{}{}{}", self.dim(&marker), task_prefix, line.text),
                    });
                } else if line.width == 0 {
                    out.push(Line::empty());
                } else {
                    out.push(Line {
                        width: indent.width() + line.width,
                        text: format!("{indent}{}", line.text),
                    });
                }
            }
        }
        out
    }

    fn render_table(
        &self,
        aligns: &[Alignment],
        head: &[Inlines],
        rows: &[Vec<Inlines>],
        width: usize,
    ) -> Vec<Line> {
        let cols = std::iter::once(head.len())
            .chain(rows.iter().map(|r| r.len()))
            .max()
            .unwrap_or(0);
        if cols == 0 {
            return Vec::new();
        }
        let natural = |cell: &Inlines| -> usize {
            self.wrap(cell, usize::MAX / 2, Style::default())
                .iter()
                .map(|l| l.width)
                .max()
                .unwrap_or(0)
        };
        let mut widths = vec![1usize; cols];
        for row in std::iter::once(head).chain(rows.iter().map(|r| r.as_slice())) {
            for (c, cell) in row.iter().enumerate() {
                widths[c] = widths[c].max(natural(cell));
            }
        }
        // Borders and padding: "| a | b |" is 3 per column plus one.
        let chrome = 3 * cols + 1;
        let budget = width.saturating_sub(chrome);
        const MIN_COL: usize = 3;
        while widths.iter().sum::<usize>() > budget {
            let (idx, widest) = widths
                .iter()
                .enumerate()
                .max_by_key(|(i, w)| (**w, usize::MAX - *i))
                .map(|(i, w)| (i, *w))
                .unwrap();
            if widest <= MIN_COL {
                break;
            }
            widths[idx] -= 1;
        }

        // trace:BUG-1979 | ai:claude
        let art = self.art;
        let border = |[left, mid, right]: [&str; 3]| -> Line {
            let text = format!(
                "{left}{}{right}",
                widths
                    .iter()
                    .map(|w| art.rule_light.repeat(w + 2))
                    .collect::<Vec<_>>()
                    .join(mid)
            );
            Line {
                width: text.width(),
                text: self.dim(&text),
            }
        };
        let render_row = |cells: &[Inlines], extra: Style| -> Vec<Line> {
            let wrapped: Vec<Vec<Line>> = (0..cols)
                .map(|c| match cells.get(c) {
                    Some(cell) => self.wrap(cell, widths[c], extra),
                    None => vec![Line::empty()],
                })
                .collect();
            let height = wrapped.iter().map(|w| w.len()).max().unwrap_or(1);
            let bar = self.dim(art.vertical);
            (0..height)
                .map(|r| {
                    let mut text = bar.clone();
                    let mut vis = 1;
                    for c in 0..cols {
                        let line = wrapped[c].get(r).cloned().unwrap_or_else(Line::empty);
                        let space = widths[c].saturating_sub(line.width);
                        let (lpad, rpad) = match aligns.get(c) {
                            Some(Alignment::Right) => (space, 0),
                            Some(Alignment::Center) => (space / 2, space - space / 2),
                            _ => (0, space),
                        };
                        text.push(' ');
                        text.push_str(&" ".repeat(lpad));
                        text.push_str(&line.text);
                        text.push_str(&" ".repeat(rpad));
                        text.push(' ');
                        text.push_str(&bar);
                        vis += line.width.max(widths[c]) + 3;
                    }
                    Line { text, width: vis }
                })
                .collect()
        };

        let mut out = vec![border(art.top)];
        out.extend(render_row(
            head,
            Style {
                bold: true,
                ..Style::default()
            },
        ));
        out.push(border(art.middle));
        for row in rows {
            out.extend(render_row(row, Style::default()));
        }
        out.push(border(art.bottom));
        out
    }
}

fn merge(a: Style, b: Style) -> Style {
    Style {
        bold: a.bold || b.bold,
        italic: a.italic || b.italic,
        code: a.code || b.code,
        strike: a.strike || b.strike,
        dim: a.dim || b.dim,
        underline: a.underline || b.underline,
    }
}

/// Split a word wider than `width` into width-sized chunks, keeping styles.
fn split_word(word: Word, width: usize) -> Vec<Word> {
    if word.width <= width {
        return vec![word];
    }
    let mut out = Vec::new();
    let mut cur = Word::default();
    for (text, style) in word.pieces {
        let mut piece = String::new();
        let mut piece_w = 0;
        for ch in text.chars() {
            let cw = ch.width().unwrap_or(0);
            if cur.width + piece_w + cw > width && cur.width + piece_w > 0 {
                if !piece.is_empty() {
                    cur.pieces.push((std::mem::take(&mut piece), style));
                    cur.width += piece_w;
                    piece_w = 0;
                }
                out.push(std::mem::take(&mut cur));
            }
            piece.push(ch);
            piece_w += cw;
        }
        if !piece.is_empty() {
            cur.pieces.push((piece, style));
            cur.width += piece_w;
        }
    }
    if !cur.pieces.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
#[path = "tests/markdown_render_tests.rs"]
mod tests;
