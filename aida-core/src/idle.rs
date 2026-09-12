//! Information-based idle detection over agent event streams.
//!
//! The classifier is intentionally small and pure: callers feed stream bytes
//! with wall-clock timestamps, and the detector decides whether the stream is
//! still producing novel information or merely redrawing the same low-entropy
//! payload. trace:STORY-998 | ai:codex

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct IdleConfig {
    pub spinning_after: Duration,
    pub runaway_rate: f64,
    pub low_entropy_bits: f64,
    pub progress_activity: Duration,
    pub max_window_lines: usize,
}

impl Default for IdleConfig {
    fn default() -> Self {
        Self {
            spinning_after: Duration::from_secs(90),
            runaway_rate: 500.0,
            low_entropy_bits: 1.5,
            progress_activity: Duration::from_secs(30),
            max_window_lines: 512,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum IdleVerdict {
    Working,
    Quiet,
    Spinning { template: String, count: usize },
    Runaway { lines_per_sec: f64 },
}

#[derive(Debug, Clone)]
struct Row {
    at: Instant,
    template: String,
    numbers: Vec<NumberToken>,
}

#[derive(Debug, Clone, Copy)]
struct NumberToken {
    value: f64,
    elapsed_clock: bool,
}

#[derive(Debug, Clone)]
pub struct IdleDetector {
    cfg: IdleConfig,
    rows: VecDeque<Row>,
    all_line_times: VecDeque<Instant>,
    last_info_at: Option<Instant>,
}

impl IdleDetector {
    pub fn new(cfg: IdleConfig) -> Self {
        Self {
            cfg,
            rows: VecDeque::new(),
            all_line_times: VecDeque::new(),
            last_info_at: None,
        }
    }

    pub fn feed_bytes(&mut self, raw: &[u8], at: Instant) -> IdleVerdict {
        self.all_line_times.push_back(at);
        let text = String::from_utf8_lossy(raw);
        let info = strip_non_information(&text);
        if !info.trim().is_empty() {
            self.last_info_at = Some(at);
            let (template, numbers) = canonicalize(&info);
            self.rows.push_back(Row {
                at,
                template,
                numbers,
            });
            while self.rows.len() > self.cfg.max_window_lines {
                self.rows.pop_front();
            }
        }
        self.prune_line_times(at);
        self.verdict(at)
    }

    pub fn feed_line(&mut self, line: &str, at: Instant) -> IdleVerdict {
        self.feed_bytes(line.as_bytes(), at)
    }

    pub fn verdict(&self, now: Instant) -> IdleVerdict {
        if let Some(rate) = self.runaway_rate(now) {
            if rate >= self.cfg.runaway_rate {
                return IdleVerdict::Runaway {
                    lines_per_sec: rate,
                };
            }
        }

        let Some(last_info) = self.last_info_at else {
            return IdleVerdict::Quiet;
        };
        if now.duration_since(last_info) >= self.cfg.progress_activity {
            return IdleVerdict::Quiet;
        }

        if let Some((template, count)) = self.spinning_template(now) {
            return IdleVerdict::Spinning { template, count };
        }
        IdleVerdict::Working
    }

    fn prune_line_times(&mut self, now: Instant) {
        let keep = self.cfg.progress_activity.max(Duration::from_secs(1));
        while self
            .all_line_times
            .front()
            .is_some_and(|t| now.duration_since(*t) > keep)
        {
            self.all_line_times.pop_front();
        }
    }

    fn runaway_rate(&self, now: Instant) -> Option<f64> {
        let first = *self.all_line_times.front()?;
        let span = now.duration_since(first).as_secs_f64().max(0.001);
        Some(self.all_line_times.len() as f64 / span)
    }

    fn spinning_template(&self, now: Instant) -> Option<(String, usize)> {
        if self.rows.len() < 6 {
            return None;
        }
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for row in &self.rows {
            *counts.entry(&row.template).or_default() += 1;
        }
        let entropy = shannon_entropy(counts.values().copied(), self.rows.len());
        if entropy > self.cfg.low_entropy_bits {
            return None;
        }
        let (template, count) = counts.into_iter().max_by_key(|(_, n)| *n)?;
        if count * 5 < self.rows.len() * 2 {
            return None;
        }
        let dominant: Vec<&Row> = self
            .rows
            .iter()
            .filter(|row| row.template == template)
            .collect();
        let first = dominant.first()?.at;
        if now.duration_since(first) < self.cfg.spinning_after {
            return None;
        }
        if numbers_are_frozen_or_clocklike(&dominant) {
            return Some((template.to_string(), count));
        }
        None
    }
}

fn shannon_entropy<I>(counts: I, total: usize) -> f64
where
    I: IntoIterator<Item = usize>,
{
    let total = total as f64;
    counts
        .into_iter()
        .filter(|n| *n > 0)
        .map(|n| {
            let p = n as f64 / total;
            -p * p.log2()
        })
        .sum()
}

fn numbers_are_frozen_or_clocklike(rows: &[&Row]) -> bool {
    let max_len = rows.iter().map(|r| r.numbers.len()).max().unwrap_or(0);
    for idx in 0..max_len {
        let series: Vec<(Instant, NumberToken)> = rows
            .iter()
            .filter_map(|r| r.numbers.get(idx).copied().map(|n| (r.at, n)))
            .collect();
        if series.len() < 2 {
            continue;
        }
        if series.iter().all(|(_, n)| n.elapsed_clock) && advances_like_elapsed_clock(&series) {
            continue;
        }
        let first = series[0].1.value;
        if series.iter().any(|(_, n)| (n.value - first).abs() > 0.0001) {
            return false;
        }
    }
    true
}

fn advances_like_elapsed_clock(series: &[(Instant, NumberToken)]) -> bool {
    let base_t = series[0].0;
    let base_v = series[0].1.value;
    series.iter().all(|(at, n)| {
        let wall = at.duration_since(base_t).as_secs_f64();
        ((n.value - base_v) - wall).abs() <= 1.5
    })
}

fn strip_non_information(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            continue;
        }
        if ch.is_control() && ch != '\n' && ch != '\t' {
            continue;
        }
        out.push(ch);
    }
    out
}

fn canonicalize(line: &str) -> (String, Vec<NumberToken>) {
    let mut template = String::new();
    let mut numbers = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch.is_ascii_digit()
            && !is_id_char(chars.get(i.wrapping_sub(1)).copied())
            && !number_is_inside_hex_id(&chars, i)
        {
            let start = i;
            let mut dot_seen = false;
            while i < chars.len() && (chars[i].is_ascii_digit() || (!dot_seen && chars[i] == '.')) {
                dot_seen |= chars[i] == '.';
                i += 1;
            }
            let raw: String = chars[start..i].iter().collect();
            let unit = chars.get(i).copied();
            let elapsed_clock = matches!(unit, Some('s' | 'S'));
            if let Ok(value) = raw.parse::<f64>() {
                numbers.push(NumberToken {
                    value,
                    elapsed_clock,
                });
            }
            template.push_str("§N");
            continue;
        }
        template.push(if ch.is_whitespace() { ' ' } else { ch });
        i += 1;
    }
    (collapse_spaces(&template), numbers)
}

fn is_id_char(ch: Option<char>) -> bool {
    ch.is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '-')
}

fn number_is_inside_hex_id(chars: &[char], idx: usize) -> bool {
    let mut start = idx;
    while start > 0 && is_hex_id_piece(chars[start - 1]) {
        start -= 1;
    }
    let mut end = idx;
    while end < chars.len() && is_hex_id_piece(chars[end]) {
        end += 1;
    }
    let token: String = chars[start..end].iter().collect();
    token.len() >= 8
        && token.contains('-')
        && token
            .chars()
            .any(|c| c.is_ascii_hexdigit() && c.is_ascii_alphabetic())
}

fn is_hex_id_piece(ch: char) -> bool {
    ch.is_ascii_hexdigit() || ch == '-'
}

fn collapse_spaces(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> IdleConfig {
        IdleConfig {
            spinning_after: Duration::from_secs(60),
            runaway_rate: 100.0,
            low_entropy_bits: 1.5,
            progress_activity: Duration::from_secs(30),
            max_window_lines: 128,
        }
    }

    #[test]
    fn distinct_uuid_stream_is_working() {
        let start = Instant::now();
        let mut d = IdleDetector::new(cfg());
        for i in 0..70 {
            let line = format!("job 01a079fe-4ad8-4d7b-9a9a-{i:012x} queued");
            d.feed_line(&line, start + Duration::from_secs(i));
        }
        assert_eq!(
            d.verdict(start + Duration::from_secs(70)),
            IdleVerdict::Working
        );
    }

    #[test]
    fn lockstep_elapsed_counter_is_spinning() {
        let start = Instant::now();
        let mut d = IdleDetector::new(cfg());
        for i in 0..70 {
            d.feed_line(
                &format!("Thinking... ({i}s · 1.2k tokens)"),
                start + Duration::from_secs(i),
            );
        }
        assert!(matches!(
            d.verdict(start + Duration::from_secs(70)),
            IdleVerdict::Spinning { .. }
        ));
    }

    #[test]
    fn high_rate_counts_all_lines_not_just_window_rows() {
        let start = Instant::now();
        let mut c = cfg();
        c.max_window_lines = 10;
        let mut d = IdleDetector::new(c);
        for i in 0..500 {
            d.feed_line("redraw", start + Duration::from_millis(i * 2));
        }
        assert!(matches!(
            d.verdict(start + Duration::from_secs(1)),
            IdleVerdict::Runaway { lines_per_sec } if lines_per_sec >= 100.0
        ));
    }

    #[test]
    fn escape_only_stream_goes_quiet() {
        let start = Instant::now();
        let mut d = IdleDetector::new(cfg());
        d.feed_line("real output", start);
        for i in 1..40 {
            d.feed_bytes(b"\x1b[2K\r", start + Duration::from_secs(i));
        }
        assert_eq!(
            d.verdict(start + Duration::from_secs(40)),
            IdleVerdict::Quiet
        );
    }

    #[test]
    fn non_utf8_input_does_not_stop_later_information() {
        let start = Instant::now();
        let mut d = IdleDetector::new(cfg());
        d.feed_bytes(b"\xff\xfe", start);
        d.feed_bytes(
            "later useful line".as_bytes(),
            start + Duration::from_secs(1),
        );
        assert_eq!(
            d.verdict(start + Duration::from_secs(2)),
            IdleVerdict::Working
        );
    }
}
