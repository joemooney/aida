//! `when = "<predicate>"` grammar for `[schedule]` jobs (STORY-1226, ADR-46
//! decision 4).
//!
//! A condition job fires when a small typed expression over the substrate
//! snapshot turns true. The grammar is deliberately tiny — no shell, no
//! presets, no free-form scripting:
//!
//! ```text
//! expr := or
//! or   := and ( '||' and )*
//! and  := atom ( '&&' atom )*
//! atom := '(' expr ')' | field op value
//! op   := '>' | '>=' | '<' | '<=' | '==' | '!='
//! value := <integer> | <duration: 90s 15m 2h 1d 1w> | true | false
//! ```
//!
//! `&&` binds tighter than `||`. Every field has a fixed type (count,
//! duration, or flag) and the parser type-checks the literal against it, so a
//! typo like `mail.unread > 15m` is rejected at config-load time, not at 3am.
//!
//! The field set is fixed and documented in `docs/cli/03-work-autonomy.md`
//! (`aida schedule`). Every field is computed from state the awaiting/status
//! readers already keep on local files — never a network call — so a `when`
//! job can be evaluated on the per-turn notice path.
//!
//! trace:STORY-1226 | ai:claude

use anyhow::{bail, Result};
use std::collections::BTreeSet;

/// The fixed, documented field set a predicate may reference.
// trace:STORY-1226 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Field {
    /// `mail.unread` — unread messages in the seat's inbox (count).
    MailUnread,
    /// `mail.oldest_unread_age` — age of the oldest unread message; `0s` when
    /// the inbox is caught up (duration).
    MailOldestUnreadAge,
    /// `drain.lock_free` — no live drain holds `.aida/drain.lock` (flag).
    DrainLockFree,
    /// `queue.drain_mode_ready` — queued specs groomed `execution_mode = drain`
    /// and not in flight (count).
    QueueDrainModeReady,
    /// `queue.depth` — queue entries routed to the seat (count).
    QueueDepth,
    /// `sessions.finished_unreaped` — finished sessions `aida session reap`
    /// would tear down (count).
    SessionsFinishedUnreaped,
    /// `ci.red_prs` — open PRs whose CI is red; `0` when the snapshot was
    /// built without network (count).
    CiRedPrs,
    /// `findings.open` — findings awaiting triage (count).
    FindingsOpen,
    /// `escalations.open` — specs parked `NeedsAttention` for a human (count).
    EscalationsOpen,
}

/// The literal type each field carries; the parser refuses a mismatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldType {
    Count,
    Duration,
    Flag,
}

impl Field {
    /// Every field, in documentation order.
    pub(crate) const ALL: [Field; 9] = [
        Field::MailUnread,
        Field::MailOldestUnreadAge,
        Field::DrainLockFree,
        Field::QueueDrainModeReady,
        Field::QueueDepth,
        Field::SessionsFinishedUnreaped,
        Field::CiRedPrs,
        Field::FindingsOpen,
        Field::EscalationsOpen,
    ];

    /// The dotted name used in config.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Field::MailUnread => "mail.unread",
            Field::MailOldestUnreadAge => "mail.oldest_unread_age",
            Field::DrainLockFree => "drain.lock_free",
            Field::QueueDrainModeReady => "queue.drain_mode_ready",
            Field::QueueDepth => "queue.depth",
            Field::SessionsFinishedUnreaped => "sessions.finished_unreaped",
            Field::CiRedPrs => "ci.red_prs",
            Field::FindingsOpen => "findings.open",
            Field::EscalationsOpen => "escalations.open",
        }
    }

    pub(crate) fn field_type(self) -> FieldType {
        match self {
            Field::MailOldestUnreadAge => FieldType::Duration,
            Field::DrainLockFree => FieldType::Flag,
            _ => FieldType::Count,
        }
    }

    fn parse(name: &str) -> Option<Field> {
        Field::ALL.iter().copied().find(|f| f.name() == name)
    }
}

/// Comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
    Ne,
}

impl Op {
    fn apply<T: PartialOrd>(self, lhs: T, rhs: T) -> bool {
        match self {
            Op::Gt => lhs > rhs,
            Op::Ge => lhs >= rhs,
            Op::Lt => lhs < rhs,
            Op::Le => lhs <= rhs,
            Op::Eq => lhs == rhs,
            Op::Ne => lhs != rhs,
        }
    }
}

/// A typed literal on the right-hand side of a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Value {
    Count(i64),
    /// Seconds.
    Duration(i64),
    Flag(bool),
}

/// One `field op value` comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Term {
    pub field: Field,
    pub op: Op,
    pub value: Value,
}

/// A parsed predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Expr {
    Term(Term),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// Every field the expression reads — lets a caller compute only the
    /// snapshot columns a registry actually references.
    pub(crate) fn fields(&self) -> BTreeSet<Field> {
        let mut out = BTreeSet::new();
        self.collect_fields(&mut out);
        out
    }

    fn collect_fields(&self, out: &mut BTreeSet<Field>) {
        match self {
            Expr::Term(t) => {
                out.insert(t.field);
            }
            Expr::And(a, b) | Expr::Or(a, b) => {
                a.collect_fields(out);
                b.collect_fields(out);
            }
        }
    }
}

/// The substrate snapshot a predicate is evaluated against. Built from
/// local-file reads only; see [`Field`] for each column's meaning.
// trace:STORY-1226 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub mail_unread: i64,
    /// Seconds; `0` when nothing is unread.
    pub mail_oldest_unread_age_secs: i64,
    pub drain_lock_free: bool,
    pub queue_drain_mode_ready: i64,
    pub queue_depth: i64,
    pub sessions_finished_unreaped: i64,
    pub ci_red_prs: i64,
    pub findings_open: i64,
    pub escalations_open: i64,
}

impl Snapshot {
    fn get(&self, field: Field) -> Value {
        match field {
            Field::MailUnread => Value::Count(self.mail_unread),
            Field::MailOldestUnreadAge => Value::Duration(self.mail_oldest_unread_age_secs),
            Field::DrainLockFree => Value::Flag(self.drain_lock_free),
            Field::QueueDrainModeReady => Value::Count(self.queue_drain_mode_ready),
            Field::QueueDepth => Value::Count(self.queue_depth),
            Field::SessionsFinishedUnreaped => Value::Count(self.sessions_finished_unreaped),
            Field::CiRedPrs => Value::Count(self.ci_red_prs),
            Field::FindingsOpen => Value::Count(self.findings_open),
            Field::EscalationsOpen => Value::Count(self.escalations_open),
        }
    }
}

/// Evaluate a parsed predicate against a snapshot. Pure. The parser already
/// type-checked every term, so a type mismatch here is an internal error.
// trace:STORY-1226 | ai:claude
pub(crate) fn eval(expr: &Expr, snap: &Snapshot) -> Result<bool> {
    Ok(match expr {
        Expr::And(a, b) => eval(a, snap)? && eval(b, snap)?,
        Expr::Or(a, b) => eval(a, snap)? || eval(b, snap)?,
        Expr::Term(t) => match (snap.get(t.field), t.value) {
            (Value::Count(l), Value::Count(r)) => t.op.apply(l, r),
            (Value::Duration(l), Value::Duration(r)) => t.op.apply(l, r),
            (Value::Flag(l), Value::Flag(r)) => match t.op {
                Op::Eq => l == r,
                Op::Ne => l != r,
                _ => bail!(
                    "predicate field `{}` is a flag; only == and != apply",
                    t.field.name()
                ),
            },
            (have, want) => bail!(
                "predicate field `{}` is {:?} but the literal is {:?}",
                t.field.name(),
                have,
                want
            ),
        },
    })
}

/// Parse a `when` predicate. Errors name the offending token and, for an
/// unknown field, list the valid set.
// trace:STORY-1226 | ai:claude
pub(crate) fn parse(input: &str) -> Result<Expr> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        bail!("empty predicate");
    }
    let mut p = Parser { tokens, pos: 0 };
    let expr = p.parse_or()?;
    if p.pos != p.tokens.len() {
        bail!(
            "unexpected token `{}` after the end of the predicate",
            p.tokens[p.pos]
        );
    }
    Ok(expr)
}

/// The valid field names, for error messages and docs.
pub(crate) fn field_names() -> Vec<&'static str> {
    Field::ALL.iter().map(|f| f.name()).collect()
}

struct Parser {
    tokens: Vec<String>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.pos).map(String::as_str)
    }

    fn next(&mut self) -> Option<String> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_or(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_and()?;
        while self.peek() == Some("||") {
            self.next();
            let rhs = self.parse_and()?;
            lhs = Expr::Or(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut lhs = self.parse_atom()?;
        while self.peek() == Some("&&") {
            self.next();
            let rhs = self.parse_atom()?;
            lhs = Expr::And(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_atom(&mut self) -> Result<Expr> {
        if self.peek() == Some("(") {
            self.next();
            let inner = self.parse_or()?;
            match self.next().as_deref() {
                Some(")") => return Ok(inner),
                other => bail!(
                    "expected `)` but found {}",
                    other
                        .map(|t| format!("`{t}`"))
                        .unwrap_or("end of predicate".into())
                ),
            }
        }
        let Some(field_tok) = self.next() else {
            bail!("expected a field name but the predicate ended");
        };
        let Some(field) = Field::parse(&field_tok) else {
            bail!(
                "unknown predicate field `{}`; valid fields: {}",
                field_tok,
                field_names().join(", ")
            );
        };
        // A bare flag (`drain.lock_free`) reads as `== true`.
        if field.field_type() == FieldType::Flag
            && !matches!(self.peek(), Some(">" | ">=" | "<" | "<=" | "==" | "!="))
        {
            return Ok(Expr::Term(Term {
                field,
                op: Op::Eq,
                value: Value::Flag(true),
            }));
        }
        let Some(op_tok) = self.next() else {
            bail!("field `{}` needs an operator and a value", field.name());
        };
        let op = match op_tok.as_str() {
            ">" => Op::Gt,
            ">=" => Op::Ge,
            "<" => Op::Lt,
            "<=" => Op::Le,
            "==" => Op::Eq,
            "!=" => Op::Ne,
            other => bail!("unknown operator `{other}` after `{}`", field.name()),
        };
        let Some(val_tok) = self.next() else {
            bail!("`{} {}` needs a value", field.name(), op_tok);
        };
        let value = parse_value(&val_tok, field)?;
        if field.field_type() == FieldType::Flag && !matches!(op, Op::Eq | Op::Ne) {
            bail!("field `{}` is a flag; only == and != apply", field.name());
        }
        Ok(Expr::Term(Term { field, op, value }))
    }
}

fn parse_value(tok: &str, field: Field) -> Result<Value> {
    match field.field_type() {
        FieldType::Flag => match tok {
            "true" => Ok(Value::Flag(true)),
            "false" => Ok(Value::Flag(false)),
            other => bail!(
                "field `{}` is a flag; expected true/false, found `{other}`",
                field.name()
            ),
        },
        FieldType::Count => match tok.parse::<i64>() {
            Ok(n) => Ok(Value::Count(n)),
            Err(_) => bail!(
                "field `{}` is a count; expected an integer, found `{tok}`",
                field.name()
            ),
        },
        FieldType::Duration => {
            if let Ok(n) = tok.parse::<i64>() {
                // Bare integer = seconds.
                return Ok(Value::Duration(n));
            }
            let d = crate::maintenance_schedule::parse_duration(tok).map_err(|e| {
                anyhow::anyhow!(
                    "field `{}` is a duration; expected e.g. 15m/2h/90s, found `{tok}` ({e})",
                    field.name()
                )
            })?;
            Ok(Value::Duration(d.num_seconds()))
        }
    }
}

fn tokenize(input: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '(' | ')' => {
                out.push(c.to_string());
                i += 1;
            }
            '&' | '|' => {
                if chars.get(i + 1) == Some(&c) {
                    out.push(format!("{c}{c}"));
                    i += 2;
                } else {
                    bail!("single `{c}` — use `&&` / `||`");
                }
            }
            '>' | '<' | '=' | '!' => {
                if chars.get(i + 1) == Some(&'=') {
                    out.push(format!("{c}="));
                    i += 2;
                } else if c == '=' || c == '!' {
                    bail!("expected `{c}=`");
                } else {
                    out.push(c.to_string());
                    i += 1;
                }
            }
            _ if c.is_ascii_alphanumeric() || c == '.' || c == '_' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
                {
                    i += 1;
                }
                out.push(chars[start..i].iter().collect());
            }
            other => bail!("unexpected character `{other}` in predicate"),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_field_op_duration() {
        let e = parse("mail.oldest_unread_age > 15m").unwrap();
        assert_eq!(
            e,
            Expr::Term(Term {
                field: Field::MailOldestUnreadAge,
                op: Op::Gt,
                value: Value::Duration(15 * 60),
            })
        );
        // Bare integer on a duration field is seconds; 90s spelled out too.
        assert_eq!(
            parse("mail.oldest_unread_age >= 90").unwrap(),
            parse("mail.oldest_unread_age >= 90s").unwrap()
        );
        // A count field refuses a duration literal.
        let err = parse("mail.unread > 15m").unwrap_err().to_string();
        assert!(err.contains("is a count"), "{err}");
    }

    #[test]
    fn and_or_precedence() {
        // a || b && c  ==  a || (b && c)
        let e = parse("mail.unread > 0 || drain.lock_free && queue.depth > 2").unwrap();
        match e {
            Expr::Or(lhs, rhs) => {
                assert!(matches!(*lhs, Expr::Term(_)));
                assert!(matches!(*rhs, Expr::And(_, _)));
            }
            other => panic!("expected Or at the root, got {other:?}"),
        }
        // Parentheses override.
        let e = parse("(mail.unread > 0 || drain.lock_free) && queue.depth > 2").unwrap();
        assert!(matches!(e, Expr::And(_, _)));
        // Fields are collected across the tree.
        let fields = e.fields();
        assert!(fields.contains(&Field::MailUnread));
        assert!(fields.contains(&Field::DrainLockFree));
        assert!(fields.contains(&Field::QueueDepth));
    }

    #[test]
    fn unknown_field_is_error() {
        let err = parse("mail.unreed > 0").unwrap_err().to_string();
        assert!(
            err.contains("unknown predicate field `mail.unreed`"),
            "{err}"
        );
        assert!(err.contains("mail.unread"), "lists the valid set: {err}");
        assert!(parse("").is_err());
        assert!(parse("mail.unread >").is_err());
        assert!(parse("mail.unread > 1 extra").is_err());
        assert!(parse("drain.lock_free > 1").is_err());
    }

    #[test]
    fn evaluates_against_snapshot() {
        let snap = Snapshot {
            mail_unread: 3,
            mail_oldest_unread_age_secs: 20 * 60,
            drain_lock_free: true,
            queue_drain_mode_ready: 2,
            ..Default::default()
        };
        let e = parse("mail.oldest_unread_age > 15m").unwrap();
        assert!(eval(&e, &snap).unwrap());
        let e = parse("drain.lock_free && queue.drain_mode_ready > 0").unwrap();
        assert!(eval(&e, &snap).unwrap());
        let e = parse("drain.lock_free == false || sessions.finished_unreaped > 0").unwrap();
        assert!(!eval(&e, &snap).unwrap());
        let e = parse("mail.unread == 3 && findings.open <= 0").unwrap();
        assert!(eval(&e, &snap).unwrap());
    }
}
