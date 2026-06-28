//! TOON (Token-Oriented Object Notation) encoder for AIDA's AGENT output mode.
//!
//! TOON is a compact, indentation + tabular encoding that is materially cheaper
//! in tokens than pretty JSON for arrays of uniform objects: a field schema is
//! declared once in the array header (`name[N]{f1,f2,f3}:`) and each row is a
//! single delimiter-joined line, so the per-row `{"key":...,"key":...}` repetition
//! JSON pays on every element disappears. See <https://toonformat.dev>.
//!
//! This is a deliberately small subset — exactly what AIDA's agent surfaces need:
//!   * scalar fields  (`key: value`)
//!   * uniform-row arrays (the tabular block above)
//!
//! It ports the gh-axi / tasks-axi `toon.ts` **FieldDef** blueprint to Rust: a
//! command declares an ordered list of [`FieldDef`]s (name + a row -> string
//! projection) and the renderer turns a slice of rows into a TOON table. The
//! per-command minimal schemas live with their commands; this module is the
//! reusable encoder + a focused decoder so the format has genuine round-trip
//! tests.
//!
//! This module is the AGENT path only — gated by `agent_output_mode()`. The human
//! emoji/table path is left byte-identical.
// trace:TASK-964 | ai:claude

/// Field delimiter inside a TOON tabular row. TOON's default is a comma.
const DELIM: char = ',';

/// A value needs quoting when emitting it bare would break parsing: it carries
/// the delimiter, a quote, a newline, a structural colon, leading or trailing
/// whitespace, or is empty (an empty bare cell is ambiguous). Mirrors the
/// conservative quoting rule in the `toon.ts` reference.
fn needs_quote(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    if value.starts_with(char::is_whitespace) || value.ends_with(char::is_whitespace) {
        return true;
    }
    value.contains(DELIM)
        || value.contains('"')
        || value.contains('\n')
        || value.contains('\r')
        || value.contains(':')
}

/// Escape one value for emission: quote-and-escape when [`needs_quote`], else
/// emit bare. Inside quotes, `\` -> `\\`, `"` -> `\"`, newline -> `\n`,
/// carriage-return -> `\r` (so a round-trip recovers the exact bytes).
pub fn escape(value: &str) -> String {
    if !needs_quote(value) {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Reverse of [`escape`] for a single cell: strip surrounding quotes and undo the
/// escapes. A bare (unquoted) cell is returned verbatim. Returns `None` on a
/// malformed quoted cell (unterminated / dangling escape).
fn unescape_cell(cell: &str) -> Option<String> {
    let cell = cell.trim();
    if !cell.starts_with('"') {
        return Some(cell.to_string());
    }
    let inner = &cell[1..];
    // Must end with a closing quote.
    if !inner.ends_with('"') {
        return None;
    }
    let body = &inner[..inner.len() - 1];
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next()? {
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                other => out.push(other),
            }
        } else {
            out.push(ch);
        }
    }
    Some(out)
}

/// Emit a single scalar field line: `key: value` (value escaped). Used for the
/// `show` / `status` head fields.
pub fn scalar(key: &str, value: &str) -> String {
    format!("{key}: {}", escape(value))
}

/// A column in a TOON table: a stable field `name` plus a projection from a row
/// `T` to its cell string. The port of the `toon.ts` `FieldDef`. The wired call
/// sites use [`table_raw`] (they project through closures that capture per-call
/// routing state, which a bare `fn` pointer can't hold); the typed [`table`] +
/// `FieldDef` pipeline is the reusable encoder API, exercised by the round-trip
/// tests.
#[allow(dead_code)]
pub struct FieldDef<T> {
    pub name: &'static str,
    pub get: fn(&T) -> String,
}

impl<T> FieldDef<T> {
    #[allow(dead_code)]
    pub const fn new(name: &'static str, get: fn(&T) -> String) -> Self {
        FieldDef { name, get }
    }
}

/// Render a uniform-row array of `rows` as a TOON table named `name`, projecting
/// each row through `fields`:
///
/// ```text
/// specs[2]{id,title,status}:
///   TASK-1,Hello,draft
///   TASK-2,"Has, comma",done
/// ```
///
/// An empty `rows` collapses to the header with a zero count and no body lines
/// (`name[0]{...}:`), which is still valid TOON and unambiguous.
#[allow(dead_code)] // typed FieldDef pipeline — exercised by the round-trip tests; wired sites use table_raw.
pub fn table<T>(name: &str, fields: &[FieldDef<T>], rows: &[T]) -> String {
    let mut out = table_header(name, fields.iter().map(|f| f.name), rows.len());
    for row in rows {
        out.push('\n');
        out.push_str("  ");
        out.push_str(&join(fields.iter().map(|f| escape(&(f.get)(row)))));
    }
    out
}

/// The header line for a table: `name[count]{field,field,...}:`.
fn table_header<'a>(
    name: &str,
    field_names: impl Iterator<Item = &'a str>,
    count: usize,
) -> String {
    let fields: Vec<&str> = field_names.collect();
    format!("{name}[{count}]{{{}}}:", fields.join(","))
}

/// Render a table from already-stringified rows (when the caller can't express
/// the projection as `fn` pointers — e.g. closures over captured state). Each
/// inner vec should have `fields.len()` cells.
pub fn table_raw(name: &str, fields: &[&str], rows: &[Vec<String>]) -> String {
    let mut out = table_header(name, fields.iter().copied(), rows.len());
    for row in rows {
        out.push('\n');
        out.push_str("  ");
        out.push_str(&join(row.iter().map(|c| escape(c))));
    }
    out
}

/// Join cells with the delimiter. Cells are assumed already-escaped.
fn join(cells: impl Iterator<Item = String>) -> String {
    cells.collect::<Vec<_>>().join(&DELIM.to_string())
}

/// A parsed TOON table, the decode target for round-trip tests.
#[allow(dead_code)] // decoder half of the encode/decode pair — exercised by the round-trip tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTable {
    pub name: String,
    pub fields: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Split one TOON row into cells on the delimiter, respecting double-quoted
/// cells (a delimiter inside quotes is literal). Quoted cells keep their quotes
/// here; [`unescape_cell`] strips them.
#[allow(dead_code)] // decoder helper — exercised by the round-trip tests.
fn split_row(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut escaped = false;
    for ch in line.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => {
                cur.push(ch);
                escaped = true;
            }
            '"' => {
                in_quotes = !in_quotes;
                cur.push(ch);
            }
            c if c == DELIM && !in_quotes => {
                cells.push(std::mem::take(&mut cur));
            }
            _ => cur.push(ch),
        }
    }
    cells.push(cur);
    cells
}

/// Parse a TOON table block (the inverse of [`table`] / [`table_raw`]) for
/// round-trip testing. Reads the `name[N]{fields}:` header then `N` indented
/// rows. Returns `None` if the header is malformed or the body is short.
#[allow(dead_code)] // decoder half of the encode/decode pair — exercised by the round-trip tests.
pub fn parse_table(input: &str) -> Option<ParsedTable> {
    let mut lines = input.lines();
    let header = lines.next()?.trim_end();
    // name[count]{f1,f2}:
    let header = header.strip_suffix(':')?;
    let (name, rest) = header.split_once('[')?;
    let (count_str, rest) = rest.split_once(']')?;
    let count: usize = count_str.parse().ok()?;
    let fields_str = rest.strip_prefix('{')?.strip_suffix('}')?;
    let fields: Vec<String> = if fields_str.is_empty() {
        Vec::new()
    } else {
        fields_str
            .split(DELIM)
            .map(|s| s.trim().to_string())
            .collect()
    };
    let mut rows = Vec::with_capacity(count);
    for _ in 0..count {
        let line = lines.next()?;
        let line = line.strip_prefix("  ").unwrap_or(line);
        let cells: Option<Vec<String>> = split_row(line).iter().map(|c| unescape_cell(c)).collect();
        rows.push(cells?);
    }
    Some(ParsedTable {
        name: name.to_string(),
        fields,
        rows,
    })
}

/// Parse a `key: value` scalar line back to `(key, value)` for round-trip tests.
#[allow(dead_code)] // decoder half of the encode/decode pair — exercised by the round-trip tests.
pub fn parse_scalar(line: &str) -> Option<(String, String)> {
    let (key, value) = line.split_once(": ")?;
    Some((key.to_string(), unescape_cell(value)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Spec {
        id: &'static str,
        title: &'static str,
        status: &'static str,
    }

    const SPEC_FIELDS: &[FieldDef<Spec>] = &[
        FieldDef::new("id", |s: &Spec| s.id.to_string()),
        FieldDef::new("title", |s: &Spec| s.title.to_string()),
        FieldDef::new("status", |s: &Spec| s.status.to_string()),
    ];

    #[test]
    fn bare_value_is_unquoted() {
        assert_eq!(escape("TASK-12"), "TASK-12");
        assert_eq!(escape("in-progress"), "in-progress");
    }

    #[test]
    fn special_values_are_quoted_and_escaped() {
        assert_eq!(escape("has, comma"), "\"has, comma\"");
        assert_eq!(escape(""), "\"\"");
        assert_eq!(escape(" leading"), "\" leading\"");
        assert_eq!(escape("a:b"), "\"a:b\"");
        assert_eq!(escape("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(escape("line1\nline2"), "\"line1\\nline2\"");
    }

    #[test]
    fn table_has_count_header_and_indented_rows() {
        let rows = vec![
            Spec {
                id: "TASK-1",
                title: "Hello",
                status: "draft",
            },
            Spec {
                id: "TASK-2",
                title: "Has, comma",
                status: "done",
            },
        ];
        let out = table("specs", SPEC_FIELDS, &rows);
        let expected = "specs[2]{id,title,status}:\n  \
                        TASK-1,Hello,draft\n  \
                        TASK-2,\"Has, comma\",done";
        assert_eq!(out, expected);
    }

    #[test]
    fn empty_table_is_zero_count_header_only() {
        let rows: Vec<Spec> = Vec::new();
        let out = table("specs", SPEC_FIELDS, &rows);
        assert_eq!(out, "specs[0]{id,title,status}:");
        // Still parses, to an empty row set.
        let parsed = parse_table(&out).unwrap();
        assert!(parsed.rows.is_empty());
        assert_eq!(parsed.fields, vec!["id", "title", "status"]);
    }

    #[test]
    fn table_round_trips_through_parse() {
        let rows = vec![
            Spec {
                id: "TASK-1",
                title: "plain",
                status: "draft",
            },
            Spec {
                id: "TASK-2",
                title: "comma, and \"quote\"",
                status: "in-progress",
            },
            Spec {
                id: "TASK-3",
                title: "",
                status: "done",
            },
        ];
        let encoded = table("specs", SPEC_FIELDS, &rows);
        let parsed = parse_table(&encoded).expect("parses");
        assert_eq!(parsed.name, "specs");
        assert_eq!(parsed.fields, vec!["id", "title", "status"]);
        assert_eq!(parsed.rows.len(), 3);
        assert_eq!(parsed.rows[1][1], "comma, and \"quote\"");
        assert_eq!(parsed.rows[2][1], "");
    }

    #[test]
    fn table_raw_matches_typed_table() {
        let typed = table(
            "queue",
            &[
                FieldDef::new("id", |s: &Spec| s.id.to_string()),
                FieldDef::new("status", |s: &Spec| s.status.to_string()),
            ],
            &[Spec {
                id: "TASK-9",
                title: "x",
                status: "draft",
            }],
        );
        let raw = table_raw(
            "queue",
            &["id", "status"],
            &[vec!["TASK-9".to_string(), "draft".to_string()]],
        );
        assert_eq!(typed, raw);
    }

    #[test]
    fn scalar_round_trips() {
        let line = scalar("title", "a multi: word value, with comma");
        let (k, v) = parse_scalar(&line).unwrap();
        assert_eq!(k, "title");
        assert_eq!(v, "a multi: word value, with comma");
    }

    #[test]
    fn scalar_bare_value_round_trips() {
        let line = scalar("status", "in-progress");
        assert_eq!(line, "status: in-progress");
        let (k, v) = parse_scalar(&line).unwrap();
        assert_eq!(k, "status");
        assert_eq!(v, "in-progress");
    }

    // Format stability: encoding the decoded form re-encodes identically
    // (idempotent), the format-test half of the acceptance.
    #[test]
    fn re_encode_is_idempotent() {
        let rows = vec![
            vec!["TASK-1".to_string(), "plain".to_string()],
            vec!["TASK-2".to_string(), "with, comma".to_string()],
        ];
        let once = table_raw("specs", &["id", "title"], &rows);
        let parsed = parse_table(&once).unwrap();
        let again = table_raw(
            &parsed.name,
            &parsed.fields.iter().map(String::as_str).collect::<Vec<_>>(),
            &parsed.rows,
        );
        assert_eq!(once, again);
    }
}
