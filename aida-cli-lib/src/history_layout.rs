//! Bounded history presentation shared by CLI and MCP.
// trace:STORY-1477 | ai:codex
use crate::history::{EventRecords, HistoryEventRecord};
use anyhow::{bail, Context, Result};
use serde::ser::{SerializeMap, Serializer};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const FIELDS: &[&str] = &[
    "commit", "date", "author", "id", "type", "priority", "title", "kind", "event", "from", "to",
    "comment",
];
const MAX_TEMPLATE: usize = 4096;
const MAX_LINE: usize = 16384;
const BUILTINS: &[(&str, &str)] = &[
    ("full", "{commit} {date} {author} {id} {event}"),
    ("oneline", "{commit} {id} {event}"),
    ("approvals", "{date:%Y-%m-%d} {id} {from} -> {to}"),
    ("compact", "{date:%H:%M} {id} {event}"),
];

#[derive(Debug, Clone)]
enum Part {
    Literal(String),
    Field(String),
    Date(String),
}
#[derive(Debug, Clone)]
pub struct Template(Vec<Part>);
#[derive(Debug, Clone)]
pub enum Layout {
    Full,
    Oneline,
    Custom(Template),
}

pub fn fields(csv: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for field in csv.split(',').map(str::trim) {
        if !FIELDS.contains(&field) || out.iter().any(|s| s == field) {
            bail!("invalid history field `{field}` (empty and duplicate fields are not allowed); valid fields: {}", FIELDS.join(","));
        }
        out.push(field.to_owned());
    }
    Ok(out)
}
fn safe(s: &str) -> String {
    s.chars().map(|c| if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') { ' ' } else { c }).collect()
}
impl Template {
    pub fn parse(raw: &str) -> Result<Self> {
        if raw.len() > MAX_TEMPLATE
            || raw
                .chars()
                .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
        {
            bail!("invalid history template: maximum 4096 bytes; literal control characters/newlines are forbidden");
        }
        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut chars = raw.chars().peekable();
        while let Some(c) = chars.next() {
            if (c == '{' || c == '}') && chars.peek() == Some(&c) {
                chars.next();
                literal.push(c);
                continue;
            }
            if c == '}' {
                bail!("invalid history template: unmatched `}}`; escape it as `}}}}`");
            }
            if c != '{' {
                literal.push(c);
                continue;
            }
            parts.push(Part::Literal(std::mem::take(&mut literal)));
            let mut token = String::new();
            loop {
                match chars.next() {
                    Some('}') => break,
                    Some('{') | None => bail!("invalid history template: unmatched brace"),
                    Some(c) => token.push(c),
                }
            }
            if let Some(fmt) = token.strip_prefix("date:") {
                if fmt.is_empty()
                    || chrono::format::StrftimeItems::new(fmt)
                        .any(|i| matches!(i, chrono::format::Item::Error))
                    || format_date("2000-01-01T00:00:00Z", fmt).is_err()
                {
                    bail!("invalid date directive `{fmt}`; use chrono strftime directives such as %Y-%m-%d or %H:%M; valid placeholders: {}", FIELDS.join(","));
                }
                parts.push(Part::Date(fmt.to_owned()));
            } else if FIELDS.contains(&token.as_str()) {
                parts.push(Part::Field(token));
            } else {
                bail!("invalid history placeholder `{token}`; valid placeholders: {} (date accepts :strftime)", FIELDS.join(","));
            }
        }
        parts.push(Part::Literal(literal));
        Ok(Self(parts))
    }
    pub fn render(&self, row: &HistoryEventRecord) -> Result<String> {
        let mut out = String::new();
        for part in &self.0 {
            let value = match part {
                Part::Literal(s) => s.clone(),
                Part::Field(f) if f == "date" => crate::history::human_timestamp(&row.timestamp),
                Part::Field(f) => cell(&value(row, f)),
                Part::Date(fmt) => format_date(&row.timestamp, fmt)?,
            };
            out.push_str(&safe(&value));
            if out.len() > MAX_LINE {
                bail!("invalid rendered history line: exceeds 16384 bytes");
            }
        }
        Ok(out)
    }
    pub fn render_records(&self, records: &EventRecords) -> Result<String> {
        let mut out = String::new();
        for row in &records.events {
            out.push_str(&self.render(row)?);
            out.push('\n');
        }
        Ok(out)
    }
}
// Some Chrono directives (for example %#z) are parsing-only. Validate a
// complete sample date before any walk, and propagate formatting errors rather
// than calling Display::to_string, which panics on fmt::Error.
fn format_date(timestamp: &str, fmt: &str) -> Result<String> {
    use chrono::{Local, NaiveDateTime, TimeZone};
    let mut out = String::new();
    // trace:SPEC-442 | ai:codex
    // Decoded feed rows already contain local minute precision. Interpret that
    // wall time locally, never as UTC; leave decoder/cache/public values intact.
    let formatted = if let Ok(date) = chrono::DateTime::parse_from_rfc3339(timestamp) {
        date.with_timezone(&Local).format(fmt).write_to(&mut out)
    } else {
        let date = NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M")
            .context("invalid event timestamp")?;
        match Local.from_local_datetime(&date).single() {
            Some(local) => local.format(fmt).write_to(&mut out),
            // A DST fold has lost its offset in the existing feed. Wall-clock
            // directives still work; offset-dependent ones fail rather than
            // guessing which instant was intended.
            None => {
                if chrono::format::StrftimeItems::new(fmt).any(|item| {
                    matches!(
                        item,
                        chrono::format::Item::Numeric(chrono::format::Numeric::Timestamp, _)
                    )
                }) {
                    bail!("invalid date directive for formatting: local event offset is ambiguous");
                }
                date.format(fmt).write_to(&mut out)
            }
        }
    };
    formatted
        .context("invalid date directive for formatting (local event offset may be ambiguous)")?;
    Ok(out)
}
fn cell(v: &Value) -> String {
    v.as_str().map(str::to_owned).unwrap_or_default()
}
/// Values are strictly event-local; never join against today's requirement.
pub fn value(r: &HistoryEventRecord, field: &str) -> Value {
    match field {
        "commit" => Value::from(r.sha.clone()),
        "date" => Value::from(r.timestamp.clone()),
        "author" => Value::from(r.author.clone()),
        "id" => Value::from(r.id.clone()),
        "type" => Value::from(r.req_type.clone()),
        "kind" => Value::from(r.kind.clone()),
        "event" => Value::from(r.summary.clone()),
        "from" => serde_json::to_value(&r.from).unwrap(),
        "to" => serde_json::to_value(&r.to).unwrap(),
        "title" => match r.kind.as_str() {
            "added" | "deleted" => r.detail["title"].clone(),
            "title_change" => r.detail["to"].clone(),
            _ => Value::Null,
        },
        "priority" => match r.kind.as_str() {
            "added" => r.detail["priority"].clone(),
            "priority_change" => r.detail["to"].clone(),
            _ => Value::Null,
        },
        "comment" if r.kind == "comments_added" => Value::from(r.summary.clone()),
        _ => Value::Null,
    }
}
// Serialize entries directly, avoiding serde_json::Map's alphabetic ordering.
struct Row<'a> {
    row: &'a HistoryEventRecord,
    fields: &'a [String],
}
impl Serialize for Row<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.fields.len()))?;
        for field in self.fields {
            map.serialize_entry(field, &value(self.row, field))?;
        }
        map.end()
    }
}
pub fn project_json(records: &EventRecords, fields: &[String]) -> Result<String> {
    #[derive(Serialize)]
    struct Envelope<'a> {
        count: usize,
        events: Vec<Row<'a>>,
        window_exhausted: bool,
        source: &'a str,
        index_tip: Option<&'a str>,
    }
    Ok(serde_json::to_string_pretty(&Envelope {
        count: records.events.len(),
        events: records
            .events
            .iter()
            .map(|row| Row { row, fields })
            .collect(),
        window_exhausted: records.window_exhausted,
        source: records.source.as_str(),
        index_tip: records.source.index_tip(),
    })?)
}
pub fn project_table(records: &EventRecords, fields: &[String], toon: bool) -> String {
    let rows: Vec<Vec<String>> = records
        .events
        .iter()
        .map(|r| fields.iter().map(|f| safe(&cell(&value(r, f)))).collect())
        .collect();
    if toon {
        return format!(
            "{}\n",
            crate::toon::table_raw(
                "events",
                &fields.iter().map(String::as_str).collect::<Vec<_>>(),
                &rows
            )
        );
    }
    let mut widths: Vec<usize> = fields.iter().map(|s| s.chars().count()).collect();
    for row in &rows {
        for (i, s) in row.iter().enumerate() {
            widths[i] = widths[i].max(s.chars().count());
        }
    }
    let mut out = String::new();
    for row in std::iter::once(fields.to_vec()).chain(rows) {
        out.push_str(
            &row.iter()
                .enumerate()
                .map(|(i, s)| format!("{s}{}", " ".repeat(widths[i] - s.chars().count())))
                .collect::<Vec<_>>()
                .join("  "),
        );
        out.push('\n');
    }
    out
}

pub struct Templates {
    user_path: PathBuf,
    project_path: PathBuf,
    user: BTreeMap<String, String>,
    project: BTreeMap<String, String>,
}
fn name(raw: &str) -> Result<(Option<&str>, &str)> {
    let (scope, name) = raw
        .split_once(':')
        .map(|(s, n)| (Some(s), n))
        .unwrap_or((None, raw));
    if scope.is_some_and(|s| !["user", "project", "builtin"].contains(&s)) {
        bail!("invalid template scope; use user:, project:, or builtin: (no global: scope)");
    }
    if name.is_empty()
        || name.len() > 64
        || !name.as_bytes()[0].is_ascii_alphabetic()
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        bail!("invalid template name `{name}`; use [A-Za-z][A-Za-z0-9_-]*, at most 64 bytes");
    }
    Ok((scope, name))
}
fn read(path: &Path) -> Result<BTreeMap<String, String>> {
    let doc = crate::config_edit::load_doc(path)?;
    let mut out = BTreeMap::new();
    if let Some(history) = doc.get("history") {
        let history = history
            .as_table_like()
            .with_context(|| format!("{}: history must be a table", path.display()))?;
        if let Some(templates) = history.get("templates") {
            let templates = templates.as_table_like().with_context(|| {
                format!("{}: history.templates must be a table", path.display())
            })?;
            for (key, val) in templates.iter() {
                let checked = || -> Result<String> {
                    let (scope, _) = name(key)?;
                    if scope.is_some() {
                        bail!("config keys must be unqualified");
                    }
                    let raw = val.as_str().context("template value must be a string")?;
                    Template::parse(raw)?;
                    Ok(raw.to_owned())
                };
                out.insert(
                    key.to_owned(),
                    checked()
                        .with_context(|| format!("{}: history.templates.{key}", path.display()))?,
                );
            }
        }
    }
    Ok(out)
}
impl Templates {
    pub fn load(root: &Path) -> Result<Self> {
        let home = crate::home_dir().context("cannot find user home for history templates")?;
        Self::at(
            home.join(".aida/config.toml"),
            root.join(".aida/config.toml"),
        )
    }
    fn at(user_path: PathBuf, project_path: PathBuf) -> Result<Self> {
        Ok(Self {
            user: read(&user_path)?,
            project: read(&project_path)?,
            user_path,
            project_path,
        })
    }
    pub fn resolve(&self, raw: &str) -> Result<Layout> {
        if raw.contains('{') {
            return Ok(Layout::Custom(Template::parse(raw)?));
        }
        let (scope, key) = name(raw)?;
        for (s, entries) in [("user", &self.user), ("project", &self.project)] {
            if scope.is_none() || scope == Some(s) {
                if let Some(raw) = entries.get(key) {
                    return Ok(Layout::Custom(Template::parse(raw)?));
                }
            }
        }
        if scope.is_none() || scope == Some("builtin") {
            if let Some((_, raw)) = BUILTINS.iter().find(|(n, _)| *n == key) {
                return Ok(match key {
                    "full" => Layout::Full,
                    "oneline" => Layout::Oneline,
                    _ => Layout::Custom(Template::parse(raw)?),
                });
            }
        }
        bail!(
            "invalid history template name `{raw}`; available templates:\n{}",
            self.list()
        )
    }
    pub fn list(&self) -> String {
        let mut out = String::from("scope:name\tformat\tshadowed-by\n");
        for (scope, entries) in [
            ("user", self.user.clone()),
            ("project", self.project.clone()),
            (
                "builtin",
                BUILTINS
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
        ] {
            for (key, raw) in entries {
                let shadow = if scope != "user" && self.user.contains_key(&key) {
                    "user"
                } else if scope == "builtin" && self.project.contains_key(&key) {
                    "project"
                } else {
                    ""
                };
                out.push_str(&format!("{scope}:{key}\t{}\t{shadow}\n", safe(&raw)));
            }
        }
        out
    }
    fn target(&self, raw: &str, removing: bool) -> Result<(&Path, String)> {
        let (scope, key) = name(raw)?;
        if removing && scope.is_none() {
            bail!("invalid removal: specify user:NAME or project:NAME explicitly");
        }
        Ok((
            match scope.unwrap_or("user") {
                "user" => &self.user_path,
                "project" => &self.project_path,
                _ => bail!("invalid mutation: builtin templates are read-only"),
            },
            key.to_owned(),
        ))
    }
    pub fn validate_save(&self, target: &str, inline: Option<&str>, force: bool) -> Result<()> {
        let raw = inline.filter(|s| s.contains('{')).context(
            "invalid save: --save-as-template requires an inline --template string on the same run",
        )?;
        Template::parse(raw)?;
        let (path, key) = self.target(target, false)?;
        if read(path)?.contains_key(&key) && !force {
            bail!("invalid save: {target} already exists; pass --force to overwrite");
        }
        Ok(())
    }
    pub fn save(&self, target: &str, raw: &str, force: bool) -> Result<()> {
        self.validate_save(target, Some(raw), force)?;
        let (path, key) = self.target(target, false)?;
        let mut doc = crate::config_edit::load_doc(path)?;
        if doc.get("history").is_none() {
            doc["history"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        if doc["history"].get("templates").is_none() {
            // trace:STORY-1477 | ai:codex
            // Inline parents serialize Values only, so a regular Table child
            // would silently disappear when the document is saved.
            doc["history"]["templates"] = if doc["history"].is_inline_table() {
                toml_edit::value(toml_edit::InlineTable::new())
            } else {
                toml_edit::Item::Table(toml_edit::Table::new())
            };
        }
        doc["history"]["templates"]
            .as_table_like_mut()
            .context("history.templates must be a table")?
            .insert(&key, toml_edit::value(raw));
        crate::config_edit::save_doc(path, &doc)?;
        self.notice(target, path);
        Ok(())
    }
    pub fn remove(&self, target: &str) -> Result<()> {
        let (path, key) = self.target(target, true)?;
        if !read(path)?.contains_key(&key) {
            bail!("invalid removal: template {target} does not exist");
        }
        let mut doc = crate::config_edit::load_doc(path)?;
        doc["history"]["templates"]
            .as_table_like_mut()
            .context("history.templates must be a table")?
            .remove(&key);
        crate::config_edit::save_doc(path, &doc)?;
        self.notice(target, path);
        Ok(())
    }
    fn notice(&self, target: &str, path: &Path) {
        let scope = name(target).ok().and_then(|v| v.0).unwrap_or("user");
        eprintln!(
            "Updated {scope} history templates: {}{}",
            path.display(),
            if scope == "project" {
                "; commit this tracked config change to share it"
            } else {
                ""
            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{event_record, Event, EventKind, HistorySource};
    fn row(kind: EventKind) -> HistoryEventRecord {
        event_record(&Event {
            sha: "123456789abcdef".into(),
            timestamp: "2026-09-26T12:34:56Z".into(),
            author: "joe".into(),
            spec_id: "TASK-1".into(),
            req_type: "Task".into(),
            kind,
        })
    }
    #[test]
    fn parser_safety_dates_and_nonrecursive_values() {
        let mut r = row(EventKind::TitleChange {
            from: "old".into(),
            to: "{id}\n\u{1b}[31m".into(),
        });
        let t = Template::parse("{{{id}}} {date:%Y} {title}").unwrap();
        assert_eq!(t.render(&r).unwrap(), "{TASK-1} 2026 {id}  [31m");
        for raw in [
            "{oops}",
            "{date:%Q}",
            "{date:%#z}",
            "{date:}",
            "{id",
            "{id}}",
            "hello\n{id}",
            "{env:HOME}",
            "{id:{title}}",
        ] {
            assert!(Template::parse(raw).is_err(), "{raw}");
        }
        assert!(Template::parse(&"x".repeat(4097)).is_err());
        assert!(!Template::parse("{date:%n}%{date:%t}")
            .unwrap()
            .render(&r)
            .unwrap()
            .contains('\n'));
        r.author = "x".repeat(MAX_LINE + 1);
        assert!(Template::parse("{author}").unwrap().render(&r).is_err());
    }
    #[test]
    fn fields_order_nulls_and_event_local_semantics() {
        let added = row(EventKind::Added {
            title: "title".into(),
            req_type: "Task".into(),
            priority: "High".into(),
        });
        assert_eq!(value(&added, "title"), "title");
        assert_eq!(value(&added, "priority"), "High");
        assert_eq!(
            value(
                &row(EventKind::Deleted {
                    title: "gone".into()
                }),
                "title"
            ),
            "gone"
        );
        assert_eq!(
            value(
                &row(EventKind::PriorityChange {
                    from: "Low".into(),
                    to: "High".into()
                }),
                "priority"
            ),
            "High"
        );
        let r = row(EventKind::StatusChange {
            from: "Draft".into(),
            to: "Approved".into(),
        });
        for f in ["title", "priority", "comment"] {
            assert!(value(&r, f).is_null());
        }
        let records = EventRecords {
            events: vec![r],
            window_exhausted: true,
            source: HistorySource::GitWalk { fallback: false },
        };
        let fs = fields("id,date,event,title").unwrap();
        let json = project_json(&records, &fs).unwrap();
        assert!(json.find("\"id\"") < json.find("\"date\""));
        assert!(json.find("\"date\"") < json.find("\"event\""));
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["events"][0].as_object().unwrap().len(), 4);
        assert!(parsed["events"][0]["title"].is_null());
        assert_eq!(parsed["window_exhausted"], true);
        assert!(project_table(&records, &fs, true).contains("{id,date,event,title}"));
        assert!(project_table(&records, &fs, false).starts_with("id"));
        for csv in ["", "id,", ",id", "id,id", "id,nope"] {
            assert!(fields(csv).is_err());
        }
    }
    fn registry(dir: &Path) -> Templates {
        Templates::at(dir.join("user.toml"), dir.join("project.toml")).unwrap()
    }
    // trace:STORY-1477 | ai:codex
    #[test]
    fn public_loader_uses_hermetic_home_and_preserves_scope() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let _env = crate::test_env::EnvVarGuard::set("HOME", home.path());
        crate::test_home::assert_hermetic(home.path());
        let load = || Templates::load(project.path()).unwrap();
        let templates = load();
        assert_eq!(templates.user_path, home.path().join(".aida/config.toml"));
        assert_eq!(
            templates.project_path,
            project.path().join(".aida/config.toml")
        );
        templates
            .save("project:mine", "project {id}", false)
            .unwrap();
        let project_bytes = std::fs::read(&templates.project_path).unwrap();
        templates.save("mine", "user {id}", false).unwrap();
        let event = row(EventKind::Deleted {
            title: "gone".into(),
        });
        let render = |name| match load().resolve(name).unwrap() {
            Layout::Custom(template) => template.render(&event).unwrap(),
            _ => panic!("expected custom template"),
        };
        assert_eq!(render("mine"), "user TASK-1");
        assert_eq!(render("project:mine"), "project TASK-1");
        load().remove("user:mine").unwrap();
        assert_eq!(render("mine"), "project TASK-1");
        assert_eq!(
            std::fs::read(&templates.project_path).unwrap(),
            project_bytes
        );
        load().remove("project:mine").unwrap();
        assert!(load().resolve("mine").is_err());
    }
    #[test]
    fn config_save_shadow_remove_and_preserve() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("project.toml"),
            "# keep me\n[other]\nx = 42 # retained\n",
        )
        .unwrap();
        let t = registry(d.path());
        assert!(matches!(t.resolve("full").unwrap(), Layout::Full));
        t.save("project:full", "{id} \"quoted\" \\ backslash", false)
            .unwrap();
        let t = registry(d.path());
        assert!(matches!(t.resolve("full").unwrap(), Layout::Custom(_)));
        assert!(matches!(t.resolve("builtin:full").unwrap(), Layout::Full));
        assert!(t
            .list()
            .contains("builtin:full\t{commit} {date} {author} {id} {event}\tproject"));
        t.save("full", "{title}", false).unwrap();
        let t = registry(d.path());
        assert!(t.list().contains("project:full\t{id}"));
        assert!(t.save("full", "{event}", false).is_err());
        t.save("full", "{event}", true).unwrap();
        for name in ["full", "builtin:full", "global:full", "user:absent"] {
            assert!(t.remove(name).is_err());
        }
        t.remove("user:full").unwrap();
        let t = registry(d.path());
        assert!(matches!(t.resolve("full").unwrap(), Layout::Custom(_)));
        t.remove("project:full").unwrap();
        let body = std::fs::read_to_string(d.path().join("project.toml")).unwrap();
        assert!(body.contains("# keep me") && body.contains("x = 42 # retained"));
        let t = registry(d.path());
        assert!(matches!(t.resolve("full").unwrap(), Layout::Full));
        for (target, raw) in [
            ("builtin:x", "{id}"),
            ("global:x", "{id}"),
            ("user:", "{id}"),
            ("user:x", "full"),
        ] {
            assert!(t.validate_save(target, Some(raw), false).is_err());
        }
        assert!(t.validate_save("x", None, false).is_err());
        assert!(t
            .resolve("missing")
            .unwrap_err()
            .to_string()
            .contains("available templates"));
    }
    #[test]
    fn malformed_config_is_never_rewritten() {
        let d = tempfile::tempdir().unwrap();
        for body in [
            "oops [",
            "history = 1",
            "[history]\ntemplates = 2",
            "[history.templates]\nx = 3",
            "[history.templates]\nx = '{bad}'",
            "[history.templates]\n'bad:name' = '{id}'",
        ] {
            let path = d.path().join("user.toml");
            std::fs::write(&path, body).unwrap();
            let err = Templates::at(path.clone(), d.path().join("project.toml"))
                .err()
                .unwrap();
            assert!(format!("{err:#}").contains("user.toml"));
            assert_eq!(std::fs::read_to_string(path).unwrap(), body);
        }
    }
    // trace:STORY-1477 | ai:codex
    #[test]
    fn inline_history_first_save_round_trip() {
        let d = tempfile::tempdir().unwrap();
        for scope in ["user", "project"] {
            let path = d.path().join(format!("{scope}.toml"));
            std::fs::write(
                &path,
                "history = { keep = 1 } # inline\n[unrelated]\nkeep = true # untouched\n",
            )
            .unwrap();
            let target = format!("{scope}:first");
            registry(d.path()).save(&target, "{id}", false).unwrap();
            assert_eq!(
                read(&path).unwrap().get("first").map(String::as_str),
                Some("{id}")
            );
            registry(d.path()).remove(&target).unwrap();
            assert!(!read(&path).unwrap().contains_key("first"));
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(
                text.contains("keep = 1")
                    && text.contains("# inline")
                    && text.contains("# untouched")
            );
        }
    }
    // trace:SPEC-442 | ai:codex
    #[test]
    fn decoded_local_date_formats_without_second_conversion() {
        assert_eq!(
            format_date("2026-09-25 23:34", "%Y-%m-%d %H:%M").unwrap(),
            "2026-09-25 23:34"
        );
        assert!(format_date("not a timestamp", "%H:%M").is_err());
        assert!(format_date("2026-09-25 23:34", "%Q").is_err());
        assert!(format_date("2026-09-25 23:34", "%#z").is_err());
    }
    #[test]
    fn inline_tables_and_worktree_roots() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("user.toml"),
            "history = { templates = { mine = '{id}' }, keep = 1 } # hi\n",
        )
        .unwrap();
        registry(d.path()).save("other", "{event}", false).unwrap();
        assert!(std::fs::read_to_string(d.path().join("user.toml"))
            .unwrap()
            .contains("keep = 1"));
        std::fs::write(
            d.path().join(".git"),
            "gitdir: /unused/main/.git/worktrees/sibling",
        )
        .unwrap();
        std::fs::create_dir(d.path().join("src")).unwrap();
        assert_eq!(
            crate::find_project_root_from(&d.path().join("src")).unwrap(),
            d.path()
        );
    }
    #[test]
    fn cli_grammar_conflicts_and_events_fields() {
        use crate::cli::{Cli, Command, HistoryCommand};
        use clap::Parser;
        for args in [
            vec!["--template", "{id}", "--fields", "id"],
            vec!["--template", "{id}", "--full"],
            vec!["--template", "{id}", "--oneline"],
            vec!["--fields", "id", "--kind", "x"],
            vec!["--force"],
            vec!["--save-as-template", "x"],
        ] {
            assert!(Cli::try_parse_from([vec!["aida", "history"], args].concat()).is_err());
        }
        let c = Cli::try_parse_from(["aida", "history", "events", "--fields", "id,date,event"])
            .unwrap();
        assert!(matches!(
            c.command,
            Command::History {
                fields: Some(_),
                cmd: Some(HistoryCommand::Events { .. }),
                ..
            }
        ));
        assert!(Cli::try_parse_from(["aida", "history", "templates", "rm", "user:mine"]).is_ok());
        assert!(Cli::try_parse_from(["aida", "history", "TASK-1", "--template", "{id}"]).is_ok());
    }
}
