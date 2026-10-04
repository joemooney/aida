//! Rebuildable, content-free per-message token ledger.
//! trace:TASK-1427 | ai:codex

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

const SCHEMA_VERSION: i64 = 1;
const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const RATE_TABLE_TOML: &str = include_str!("../../data/token-rates/v1.toml");

// trace:TASK-1434 | ai:codex
#[derive(Debug, Clone, Deserialize)]
struct RateTable {
    schema_version: u32,
    table_version: String,
    published_at: String,
    currency: String,
    unit_tokens: u64,
    source_url: String,
    source_revision: String,
    source_retrieved_at: String,
    license: String,
    rates: Vec<RateEntry>,
    #[serde(default)]
    aliases: Vec<RateAlias>,
}

#[derive(Debug, Clone, Deserialize)]
struct RateEntry {
    id: String,
    provider: String,
    model: String,
    effective_from: String,
    effective_to: Option<String>,
    currency: String,
    source_url: String,
    source_revision: String,
    source_retrieved_at: String,
    input_uncached: Option<String>,
    input_cache_write: Option<String>,
    input_cache_read: Option<String>,
    output: Option<String>,
    output_reasoning: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RateAlias {
    id: String,
    provider: String,
    alias: String,
    canonical_model: String,
    effective_from: String,
    effective_to: Option<String>,
    source_url: String,
    source_revision: String,
    source_retrieved_at: String,
}

#[derive(Debug, Clone)]
struct MatchedRate<'a> {
    entry: &'a RateEntry,
    resolved_model: &'a str,
    alias_id: Option<&'a str>,
}

fn parse_rate_pico_per_token(value: &str) -> Result<i128> {
    let value = value.trim();
    if value.starts_with('-') || value.starts_with('+') || value.is_empty() {
        bail!("rate must be a non-negative decimal");
    }
    let mut pieces = value.split('.');
    let whole = pieces.next().unwrap_or("");
    let fraction = pieces.next().unwrap_or("");
    if pieces.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 6
    {
        bail!("rate must have at most six decimal places in USD per million tokens");
    }
    let whole: i128 = whole.parse()?;
    let fraction: i128 = if fraction.is_empty() {
        0
    } else {
        format!("{fraction:0<6}").parse()?
    };
    whole
        .checked_mul(1_000_000)
        .and_then(|n| n.checked_add(fraction))
        .ok_or_else(|| anyhow::anyhow!("rate overflows fixed-point representation"))
}

fn parse_rate_time(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}

fn active_interval(at: DateTime<Utc>, from: &str, to: Option<&str>) -> Result<bool> {
    let from = parse_rate_time(from)?;
    let to = to.map(parse_rate_time).transpose()?;
    Ok(at >= from && to.is_none_or(|end| at < end))
}

fn validate_rate_table(table: &RateTable) -> Result<()> {
    if table.schema_version != 1 || table.currency != "USD" || table.unit_tokens != 1_000_000 {
        bail!("unsupported rate table schema, currency, or token unit");
    }
    if table.rates.len() > 1_000 || table.aliases.len() > 1_000 {
        bail!("rate table exceeds bounded entry count");
    }
    let mut ids = BTreeSet::new();
    for rate in &table.rates {
        if !ids.insert(rate.id.as_str())
            || rate.provider.len() > 128
            || rate.model.len() > 256
            || rate.currency != table.currency
        {
            bail!("duplicate or oversized rate identity");
        }
        if !rate.source_url.starts_with("https://")
            || rate.source_revision.trim().is_empty()
            || parse_rate_time(&rate.source_retrieved_at).is_err()
        {
            bail!("rate entry lacks valid provenance");
        }
        parse_rate_time(&rate.effective_from)?;
        if let Some(to) = rate.effective_to.as_deref() {
            if parse_rate_time(to)? <= parse_rate_time(&rate.effective_from)? {
                bail!("invalid effective interval for {}", rate.id);
            }
        }
        for value in [
            &rate.input_uncached,
            &rate.input_cache_write,
            &rate.input_cache_read,
            &rate.output,
            &rate.output_reasoning,
        ]
        .into_iter()
        .flatten()
        {
            parse_rate_pico_per_token(value)?;
        }
    }
    if !table.source_url.starts_with("https://") || table.license.trim().is_empty() {
        bail!("rate table requires HTTPS provenance and license metadata");
    }
    for alias in &table.aliases {
        if !ids.insert(alias.id.as_str()) || alias.alias.len() > 256 {
            bail!("duplicate or oversized alias identity");
        }
        if !alias.source_url.starts_with("https://")
            || alias.source_revision.trim().is_empty()
            || parse_rate_time(&alias.source_retrieved_at).is_err()
        {
            bail!("alias entry lacks valid provenance");
        }
        parse_rate_time(&alias.effective_from)?;
        if let Some(to) = alias.effective_to.as_deref() {
            if parse_rate_time(to)? <= parse_rate_time(&alias.effective_from)? {
                bail!("invalid alias interval for {}", alias.id);
            }
        }
    }
    for (index, left) in table.rates.iter().enumerate() {
        for right in table.rates.iter().skip(index + 1) {
            if left.provider == right.provider
                && left.model == right.model
                && intervals_overlap(
                    &left.effective_from,
                    left.effective_to.as_deref(),
                    &right.effective_from,
                    right.effective_to.as_deref(),
                )?
            {
                bail!(
                    "overlapping rate intervals for {}/{}",
                    left.provider,
                    left.model
                );
            }
        }
    }
    for (index, left) in table.aliases.iter().enumerate() {
        for right in table.aliases.iter().skip(index + 1) {
            if left.provider == right.provider
                && left.alias == right.alias
                && intervals_overlap(
                    &left.effective_from,
                    left.effective_to.as_deref(),
                    &right.effective_from,
                    right.effective_to.as_deref(),
                )?
            {
                bail!(
                    "overlapping alias intervals for {}/{}",
                    left.provider,
                    left.alias
                );
            }
        }
    }
    Ok(())
}

fn intervals_overlap(
    left_from: &str,
    left_to: Option<&str>,
    right_from: &str,
    right_to: Option<&str>,
) -> Result<bool> {
    let left_from = parse_rate_time(left_from)?;
    let right_from = parse_rate_time(right_from)?;
    let left_to = left_to.map(parse_rate_time).transpose()?;
    let right_to = right_to.map(parse_rate_time).transpose()?;
    Ok(left_to.is_none_or(|end| right_from < end) && right_to.is_none_or(|end| left_from < end))
}

fn load_rate_table() -> Result<RateTable> {
    let table: RateTable = toml::from_str(RATE_TABLE_TOML)?;
    validate_rate_table(&table)?;
    Ok(table)
}

fn match_rate<'a>(
    table: &'a RateTable,
    provider: &str,
    raw_model: &str,
    at: DateTime<Utc>,
) -> Result<MatchedRate<'a>, String> {
    let direct_known = table
        .rates
        .iter()
        .filter(|r| r.provider == provider && r.model == raw_model)
        .collect::<Vec<_>>();
    let direct: Vec<_> = direct_known
        .iter()
        .copied()
        .filter(|r| {
            active_interval(at, &r.effective_from, r.effective_to.as_deref()).unwrap_or(false)
        })
        .collect();
    if direct.len() == 1 {
        return Ok(MatchedRate {
            entry: direct[0],
            resolved_model: direct[0].model.as_str(),
            alias_id: None,
        });
    }
    if direct.len() > 1 {
        return Err("ambiguous_rate_interval".into());
    }
    if !direct_known.is_empty() {
        return Err("rate_out_of_range".into());
    }
    let alias_known = table
        .aliases
        .iter()
        .filter(|a| a.provider == provider && a.alias == raw_model)
        .collect::<Vec<_>>();
    let aliases: Vec<_> = alias_known
        .iter()
        .copied()
        .filter(|a| {
            active_interval(at, &a.effective_from, a.effective_to.as_deref()).unwrap_or(false)
        })
        .collect();
    if aliases.len() > 1 {
        return Err("ambiguous_alias".into());
    }
    let Some(alias) = aliases.first() else {
        return Err(if alias_known.is_empty() {
            "unknown_model"
        } else {
            "alias_out_of_range"
        }
        .into());
    };
    let resolved: Vec<_> = table
        .rates
        .iter()
        .filter(|r| r.provider == provider && r.model == alias.canonical_model)
        .filter(|r| {
            active_interval(at, &r.effective_from, r.effective_to.as_deref()).unwrap_or(false)
        })
        .collect();
    if resolved.len() != 1 {
        return Err(if resolved.is_empty() {
            "alias_target_out_of_range"
        } else {
            "ambiguous_rate_interval"
        }
        .into());
    }
    Ok(MatchedRate {
        entry: resolved[0],
        resolved_model: resolved[0].model.as_str(),
        alias_id: Some(alias.id.as_str()),
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
struct Tokens {
    input_uncached: Option<u64>,
    input_cache_write: Option<u64>,
    input_cache_read: Option<u64>,
    output: Option<u64>,
    output_reasoning: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct Record {
    record_id: String,
    vendor: String,
    vendor_session_id: String,
    vendor_event_id: String,
    occurred_at: Option<String>,
    ordinal: Option<i64>,
    raw_model: Option<String>,
    source_basename: String,
    source_hash: String,
    collection_status: String,
    status_reason: Option<String>,
    tokens: Tokens,
    vendor_usage: Value,
    attribution: Attribution,
}

#[derive(Debug, Clone, Default, Serialize)]
struct Attribution {
    aida_session_id: Option<String>,
    run_uuid: Option<String>,
    spec_id: Option<String>,
    seat: Option<String>,
    phase: Option<String>,
    round: Option<u32>,
    session_reason: Option<String>,
    run_reason: Option<String>,
    spec_reason: Option<String>,
    seat_reason: Option<String>,
    phase_reason: Option<String>,
    round_reason: Option<String>,
}

#[derive(Debug, Default, Serialize)]
struct Coverage {
    source_files: usize,
    candidate_events: usize,
    measured_unique: usize,
    duplicates_suppressed: usize,
    conflicts: usize,
    truncated: usize,
    unsupported: usize,
    unattributed: usize,
}

#[derive(Debug, Clone)]
struct ManifestEvidence {
    aida_session_id: String,
    vendor_session_id: String,
    items: Vec<ManifestItem>,
    role: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestItem {
    spec_id: String,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct PhaseInterval {
    spec: String,
    run_uuid: String,
    phase: String,
    round: Option<u32>,
    round_reason: Option<String>,
    start: DateTime<Utc>,
    end: Option<DateTime<Utc>>,
}

fn ledger_path(root: &Path) -> PathBuf {
    root.join(".aida/derived/token-ledger-v1.sqlite")
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn stable_id(parts: &[&str]) -> String {
    sha(parts.join("\u{1f}").as_bytes())
}

fn project_identity(root: &Path) -> String {
    let remote = Command::new("git")
        .args([
            "-C",
            root.to_string_lossy().as_ref(),
            "config",
            "--get",
            "remote.origin.url",
        ])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    sha(format!("{}\n{remote}", root.display()).as_bytes())
}

fn u64_at(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

fn canonical_usage(v: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for key in [
        "input_tokens",
        "cached_input_tokens",
        "cache_write_input_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
        "output_tokens",
        "reasoning_output_tokens",
        "total_tokens",
    ] {
        if let Some(n) = u64_at(v, key) {
            out.insert(key.to_string(), Value::from(n));
        }
    }
    if let Some(n) = v
        .pointer("/output_tokens_details/thinking_tokens")
        .and_then(Value::as_u64)
    {
        out.insert("thinking_tokens".into(), Value::from(n));
    }
    Value::Object(out)
}

fn hash_regular_source(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("source is not a regular file: {}", path.display());
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        bail!(
            "source exceeds {} bytes: {}",
            MAX_SOURCE_BYTES,
            path.display()
        );
    }
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn bounded_lines(path: &Path, mut visit: impl FnMut(&[u8])) -> Result<usize> {
    let mut reader = BufReader::with_capacity(64 * 1024, fs::File::open(path)?);
    let mut line = Vec::new();
    let mut oversized = false;
    let mut rejected = 0usize;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if oversized {
                rejected += 1;
            } else if !line.is_empty() {
                visit(&line);
            }
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |idx| idx + 1);
        if !oversized {
            if line.len() + take > MAX_LINE_BYTES {
                oversized = true;
                line.clear();
            } else {
                line.extend_from_slice(&available[..take]);
            }
        }
        reader.consume(take);
        if newline.is_some() {
            if oversized {
                rejected += 1;
            } else {
                visit(&line);
            }
            line.clear();
            oversized = false;
        }
    }
    Ok(rejected)
}

fn file_contains_codex_marker(path: &Path) -> Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES
    {
        return Ok(false);
    }
    let mut reader = BufReader::new(fs::File::open(path)?);
    let mut buffer = [0u8; 64 * 1024];
    let markers = [
        b"\"token_count\"".as_slice(),
        b"\"turn.completed\"".as_slice(),
        b"\"token_usage_record\"".as_slice(),
    ];
    let mut overlap = Vec::new();
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(false);
        }
        overlap.extend_from_slice(&buffer[..read]);
        if markers
            .iter()
            .any(|m| overlap.windows(m.len()).any(|w| w == *m))
        {
            return Ok(true);
        }
        let keep = markers.iter().map(|m| m.len()).max().unwrap_or(1) - 1;
        if overlap.len() > keep {
            overlap.drain(..overlap.len() - keep);
        }
    }
}

fn prepare_ledger_destination(root: &Path) -> Result<(PathBuf, PathBuf)> {
    let aida = root.join(".aida");
    let parent = aida.join("derived");
    let dest = parent.join("token-ledger-v1.sqlite");
    for path in [&aida, &parent, &dest] {
        if let Ok(meta) = fs::symlink_metadata(path) {
            if meta.file_type().is_symlink() {
                bail!(
                    "refusing symlinked ledger destination component: {}",
                    path.display()
                );
            }
        }
    }
    fs::create_dir_all(&parent)?;
    let canonical_parent = parent.canonicalize()?;
    if !canonical_parent.starts_with(root) {
        bail!("ledger destination escapes project root");
    }
    Ok((parent, dest))
}

fn parse_claude_file(path: &Path, coverage: &mut Coverage) -> Result<Vec<Record>> {
    let source_hash = hash_regular_source(path)?;
    let basename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let mut by_key: BTreeMap<String, Record> = BTreeMap::new();
    let rejected = bounded_lines(path, |line| {
        let Ok(v) = serde_json::from_slice::<Value>(line) else {
            coverage.truncated += 1;
            return;
        };
        let Some(usage) = v.pointer("/message/usage") else {
            return;
        };
        coverage.candidate_events += 1;
        let session = v.get("session_id").and_then(Value::as_str).unwrap_or("");
        let message = v.pointer("/message/id").and_then(Value::as_str);
        let request = v.get("requestId").and_then(Value::as_str);
        let outer = v.get("uuid").and_then(Value::as_str);
        let event_id = match (message, request, outer) {
            (Some(m), Some(q), _) => format!("{m}:{q}"),
            (_, _, Some(o)) => o.to_string(),
            _ => {
                coverage.unsupported += 1;
                return;
            }
        };
        if session.is_empty() {
            coverage.unsupported += 1;
            return;
        }
        let native = canonical_usage(usage);
        let usage_hash = sha(serde_json::to_string(&native).unwrap().as_bytes());
        let key = stable_id(&["claude", session, &event_id]);
        let record = Record {
            record_id: key.clone(),
            vendor: "claude".into(),
            vendor_session_id: session.into(),
            vendor_event_id: event_id,
            occurred_at: v
                .get("timestamp")
                .and_then(Value::as_str)
                .map(str::to_string),
            ordinal: None,
            raw_model: v
                .pointer("/message/model")
                .and_then(Value::as_str)
                .map(str::to_string),
            source_basename: basename.clone(),
            source_hash: source_hash.clone(),
            collection_status: "measured".into(),
            status_reason: None,
            tokens: Tokens {
                input_uncached: u64_at(usage, "input_tokens"),
                input_cache_write: u64_at(usage, "cache_creation_input_tokens"),
                input_cache_read: u64_at(usage, "cache_read_input_tokens"),
                output: u64_at(usage, "output_tokens"),
                output_reasoning: usage
                    .pointer("/output_tokens_details/thinking_tokens")
                    .and_then(Value::as_u64),
            },
            vendor_usage: native,
            attribution: Attribution::default(),
        };
        match by_key.get_mut(&key) {
            None => {
                by_key.insert(key, record);
            }
            Some(existing)
                if sha(serde_json::to_string(&existing.vendor_usage)
                    .unwrap()
                    .as_bytes())
                    == usage_hash =>
            {
                coverage.duplicates_suppressed += 1;
            }
            Some(existing) => {
                existing.collection_status = "conflict".into();
                existing.status_reason = Some("same_identity_conflicting_usage".into());
                existing.tokens = Tokens::default();
                coverage.conflicts += 1;
            }
        }
    })?;
    coverage.truncated += rejected;
    Ok(by_key.into_values().collect())
}

fn parse_codex_file(path: &Path, coverage: &mut Coverage) -> Result<Vec<Record>> {
    let source_hash = hash_regular_source(path)?;
    let basename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let default_session = basename.trim_end_matches(".jsonl");
    let mut session = default_session.to_string();
    let mut segment = 0u32;
    let mut last_total: Option<u64> = None;
    let mut primary: BTreeMap<String, Record> = BTreeMap::new();
    let mut fallback: BTreeMap<String, Record> = BTreeMap::new();
    let mut seen_checkpoints = BTreeSet::new();
    let mut line_number = 0u64;
    let rejected = bounded_lines(path, |line| {
        line_number += 1;
        let Ok(v) = serde_json::from_slice::<Value>(line) else {
            coverage.truncated += 1;
            return;
        };
        if v.get("type").and_then(Value::as_str) == Some("session_meta") {
            if let Some(id) = v.pointer("/payload/id").and_then(Value::as_str) {
                session = id.into();
            }
        }
        if v.get("type").and_then(Value::as_str) == Some("token_usage_record") {
            coverage.candidate_events += 1;
            let payload = &v["payload"];
            let Some(response_id) = payload.get("response_id").and_then(Value::as_str) else {
                coverage.unsupported += 1;
                return;
            };
            let Some(usage) = payload.get("usage") else {
                coverage.unsupported += 1;
                return;
            };
            let vendor_session = payload
                .get("session_id")
                .or_else(|| payload.get("thread_id"))
                .and_then(Value::as_str)
                .unwrap_or(&session);
            let key = stable_id(&["codex", vendor_session, response_id]);
            let input = u64_at(usage, "input_tokens");
            let cached = u64_at(usage, "cached_input_tokens");
            let record = Record {
                record_id: key.clone(),
                vendor: "codex".into(),
                vendor_session_id: vendor_session.into(),
                vendor_event_id: response_id.into(),
                occurred_at: v
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                ordinal: v.get("ordinal").and_then(Value::as_i64),
                raw_model: None,
                source_basename: basename.clone(),
                source_hash: source_hash.clone(),
                collection_status: "measured".into(),
                status_reason: None,
                tokens: Tokens {
                    input_uncached: match (input, cached) {
                        (Some(i), Some(c)) if i >= c => Some(i - c),
                        (Some(i), None) => Some(i),
                        _ => None,
                    },
                    input_cache_write: u64_at(usage, "cache_write_input_tokens"),
                    input_cache_read: cached,
                    output: u64_at(usage, "output_tokens"),
                    output_reasoning: u64_at(usage, "reasoning_output_tokens"),
                },
                vendor_usage: canonical_usage(usage),
                attribution: Attribution::default(),
            };
            match primary.get_mut(&key) {
                None => {
                    primary.insert(key, record);
                }
                Some(existing) if existing.vendor_usage == record.vendor_usage => {
                    coverage.duplicates_suppressed += 1
                }
                Some(existing) => {
                    existing.collection_status = "conflict".into();
                    existing.status_reason = Some("same_response_id_conflicting_usage".into());
                    existing.tokens = Tokens::default();
                    coverage.conflicts += 1;
                }
            }
            return;
        }
        let (last, total, is_turn_completed) =
            if v.pointer("/payload/type").and_then(Value::as_str) == Some("token_count") {
                let Some(last) = v.pointer("/payload/info/last_token_usage") else {
                    coverage.unsupported += 1;
                    return;
                };
                let Some(total) = v.pointer("/payload/info/total_token_usage") else {
                    coverage.unsupported += 1;
                    return;
                };
                (last, total, false)
            } else if v.get("type").and_then(Value::as_str) == Some("turn.completed") {
                let Some(usage) = v.get("usage") else {
                    coverage.unsupported += 1;
                    return;
                };
                (usage, usage, true)
            } else {
                return;
            };
        coverage.candidate_events += 1;
        let total_n = u64_at(total, "total_tokens").unwrap_or(0);
        if last_total.is_some_and(|n| total_n < n) {
            segment += 1;
            seen_checkpoints.clear();
        }
        last_total = Some(total_n);
        let checkpoint = serde_json::to_string(&canonical_usage(total)).unwrap();
        if !seen_checkpoints.insert(checkpoint) {
            coverage.duplicates_suppressed += 1;
            return;
        }
        let event_id = if is_turn_completed {
            format!("turn:{segment}:line-{line_number}")
        } else {
            let ordinal = v
                .get("ordinal")
                .and_then(Value::as_i64)
                .unwrap_or(line_number as i64);
            format!("checkpoint:{segment}:ordinal-{ordinal}")
        };
        let key = stable_id(&["codex", &session, &event_id]);
        let input = u64_at(last, "input_tokens");
        let cached = u64_at(last, "cached_input_tokens");
        let record = Record {
            record_id: key.clone(),
            vendor: "codex".into(),
            vendor_session_id: session.clone(),
            vendor_event_id: event_id,
            occurred_at: v
                .get("timestamp")
                .and_then(Value::as_str)
                .map(str::to_string),
            ordinal: v.get("ordinal").and_then(Value::as_i64),
            raw_model: None,
            source_basename: basename.clone(),
            source_hash: source_hash.clone(),
            collection_status: "measured".into(),
            status_reason: None,
            tokens: Tokens {
                input_uncached: match (input, cached) {
                    (Some(i), Some(c)) if i >= c => Some(i - c),
                    (Some(i), None) => Some(i),
                    _ => None,
                },
                input_cache_write: u64_at(last, "cache_write_input_tokens"),
                input_cache_read: cached,
                output: u64_at(last, "output_tokens"),
                output_reasoning: u64_at(last, "reasoning_output_tokens"),
            },
            vendor_usage: canonical_usage(last),
            attribution: Attribution::default(),
        };
        match fallback.get_mut(&key) {
            None => {
                fallback.insert(key, record);
            }
            Some(existing) if existing.vendor_usage == record.vendor_usage => {
                coverage.duplicates_suppressed += 1
            }
            Some(existing) => {
                existing.collection_status = "conflict".into();
                existing.status_reason = Some("same_checkpoint_conflicting_last_usage".into());
                existing.tokens = Tokens::default();
                coverage.conflicts += 1;
            }
        }
    })?;
    coverage.truncated += rejected;
    // Stable response IDs win only when the legacy representation can be
    // correlated to that same request. Mixed-version logs can contain older,
    // distinct requests which must remain visible. trace:TASK-1427 | ai:codex
    let mut records: Vec<Record> = primary.into_values().collect();
    for (_, mut legacy) in fallback {
        let matched = records.iter().any(|stable| {
            stable.vendor_session_id == legacy.vendor_session_id
                && stable.vendor_usage == legacy.vendor_usage
                && ((stable.occurred_at.is_some() && stable.occurred_at == legacy.occurred_at)
                    || (stable.ordinal.is_some() && stable.ordinal == legacy.ordinal))
        });
        if matched {
            coverage.duplicates_suppressed += 1;
        } else {
            if !records.is_empty() {
                legacy.status_reason = Some("unmatched_legacy_fallback".into());
            }
            records.push(legacy);
        }
    }
    records.sort_by(|a, b| a.record_id.cmp(&b.record_id));
    Ok(records)
}

fn walk_jsonl(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        // Vendor roots are user-controlled local state. Never escape them via
        // a symlink while deriving telemetry. trace:TASK-1427 | ai:codex
        if kind.is_symlink() {
            continue;
        } else if kind.is_dir() {
            walk_jsonl(&p, out);
        } else if kind.is_file() && p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            out.push(p);
        }
    }
}

fn load_manifests(root: &Path) -> Vec<ManifestEvidence> {
    let dir = root.join(".aida/sessions");
    let mut leases: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("toml")
                || p.to_string_lossy().contains("manifest")
                || p.to_string_lossy().contains("activity")
            {
                continue;
            }
            if let Ok(text) = fs::read_to_string(&p) {
                if let Ok(v) = toml::from_str::<toml::Value>(&text) {
                    if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
                        leases.insert(
                            id.to_string(),
                            (
                                v.get("role").and_then(|x| x.as_str()).map(str::to_string),
                                v.get("scope").and_then(|x| x.as_str()).map(str::to_string),
                            ),
                        );
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if !p.to_string_lossy().ends_with(".manifest.toml") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&p) else {
            continue;
        };
        let Ok(v) = toml::from_str::<toml::Value>(&text) else {
            continue;
        };
        let (Some(aida), Some(vendor)) = (
            v.get("session_id").and_then(|x| x.as_str()),
            v.get("claude_session_id").and_then(|x| x.as_str()),
        ) else {
            continue;
        };
        let items = v
            .get("items")
            .and_then(|x| x.clone().try_into().ok())
            .unwrap_or_default();
        out.push(ManifestEvidence {
            aida_session_id: aida.into(),
            vendor_session_id: vendor.into(),
            items,
            role: leases.get(aida).and_then(|(role, _)| role.clone()),
        });
    }
    // Orchestrated headless phases retain an exact vendor-session -> lease
    // receipt even when no pickup manifest was written.
    if let Ok(entries) = fs::read_dir(root.join(".aida/orchestrator-handoffs")) {
        for entry in entries.flatten() {
            let Ok(text) = fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let (Some(vendor), Some(lease_id)) = (
                value.get("claude_session_id").and_then(Value::as_str),
                value.get("lease_id").and_then(Value::as_str),
            ) else {
                continue;
            };
            if out.iter().any(|m| m.vendor_session_id == vendor) {
                continue;
            }
            let Some((role, scope)) = leases.get(lease_id) else {
                continue;
            };
            let items = scope
                .as_ref()
                .filter(|s| s.contains('-'))
                .map(|spec| {
                    vec![ManifestItem {
                        spec_id: spec.clone(),
                        started_at: None,
                        completed_at: None,
                    }]
                })
                .unwrap_or_default();
            out.push(ManifestEvidence {
                aida_session_id: lease_id.into(),
                vendor_session_id: vendor.into(),
                items,
                role: role.clone(),
            });
        }
    }
    out
}

fn load_phases(root: &Path) -> Vec<PhaseInterval> {
    let path = root.join(".aida/events.jsonl");
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    let mut raw = Vec::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if v.pointer("/kind/event").and_then(Value::as_str) != Some("PhaseEntered") {
            continue;
        }
        let (Some(ts), Some(spec), Some(run), Some(phase)) = (
            v.get("ts").and_then(Value::as_str),
            v.get("spec").and_then(Value::as_str),
            v.get("run_uuid").and_then(Value::as_str),
            v.pointer("/kind/slug").and_then(Value::as_str),
        ) else {
            continue;
        };
        if let Ok(ts) = ts.parse::<DateTime<Utc>>() {
            let (attempt, round_reason) = match v.pointer("/kind/attempt") {
                Some(value) => match value.as_u64().and_then(|n| u32::try_from(n).ok()) {
                    Some(n) if n > 0 => (Some(n), None),
                    _ => (None, Some("malformed_attempt".to_string())),
                },
                None => (None, Some("missing_attempt".to_string())),
            };
            raw.push((
                ts,
                spec.to_string(),
                run.to_string(),
                phase.to_string(),
                attempt,
                round_reason,
            ));
        }
    }
    raw.sort();
    let mut out = Vec::new();
    for (idx, (start, spec, run, phase, attempt, round_reason)) in raw.iter().enumerate() {
        // Equal-timestamp entries are concurrent evidence, not a zero-length
        // predecessor. Keeping both intervals makes attribution explicitly
        // ambiguous instead of selecting by file order. trace:TASK-1427 | ai:codex
        let end = raw[idx + 1..]
            .iter()
            .find(|(next, s, r, _, _, _)| s == spec && r == run && next > start)
            .map(|x| x.0);
        out.push(PhaseInterval {
            spec: spec.clone(),
            run_uuid: run.clone(),
            phase: phase.clone(),
            round: *attempt,
            round_reason: round_reason.clone(),
            start: *start,
            end,
        });
    }
    out
}

fn attribute(
    records: &mut [Record],
    manifests: &[ManifestEvidence],
    phases: &[PhaseInterval],
    coverage: &mut Coverage,
) {
    for record in records {
        let candidates: Vec<_> = manifests
            .iter()
            .filter(|m| {
                m.vendor_session_id == record.vendor_session_id
                    || record.source_basename.contains(&m.vendor_session_id)
            })
            .collect();
        if candidates.len() != 1 {
            let reason = if candidates.is_empty() {
                "no_exact_session"
            } else {
                "multiple_candidates"
            };
            record.attribution.session_reason = Some(reason.into());
            record.attribution.run_reason = Some(reason.into());
            record.attribution.spec_reason = Some(reason.into());
            record.attribution.seat_reason = Some(reason.into());
            record.attribution.phase_reason = Some(reason.into());
            record.attribution.round_reason = Some(reason.into());
            coverage.unattributed += 1;
            continue;
        }
        let m = candidates[0];
        record.attribution.aida_session_id = Some(m.aida_session_id.clone());
        record.attribution.seat = m.role.clone();
        if m.role.is_none() {
            record.attribution.seat_reason = Some("no_lease_role".into());
        }
        let ts = record
            .occurred_at
            .as_deref()
            .and_then(|s| s.parse::<DateTime<Utc>>().ok());
        let item_candidates: Vec<_> = m
            .items
            .iter()
            .filter(|i| {
                if m.items.len() == 1 {
                    return true;
                }
                match ts {
                    Some(t) => {
                        i.started_at.is_some_and(|s| s <= t) && i.completed_at.is_none_or(|e| t < e)
                    }
                    None => false,
                }
            })
            .collect();
        if item_candidates.len() != 1 {
            let reason = if item_candidates.is_empty() {
                "missing_interval"
            } else {
                "multiple_candidates"
            };
            record.attribution.spec_reason = Some(reason.into());
            record.attribution.run_reason = Some(reason.into());
            record.attribution.phase_reason = Some(reason.into());
            record.attribution.round_reason = Some(reason.into());
            coverage.unattributed += 1;
            continue;
        }
        let spec = item_candidates[0].spec_id.to_ascii_uppercase();
        record.attribution.spec_id = Some(spec.clone());
        let phase_candidates: Vec<_> = phases
            .iter()
            .filter(|p| {
                p.spec.eq_ignore_ascii_case(&spec)
                    && ts.is_some_and(|t| p.start <= t && p.end.is_none_or(|e| t < e))
            })
            .collect();
        if phase_candidates.len() == 1 {
            let p = phase_candidates[0];
            record.attribution.run_uuid = Some(p.run_uuid.clone());
            record.attribution.phase = Some(p.phase.clone());
            record.attribution.round = p.round;
            record.attribution.round_reason = p.round_reason.clone();
        } else {
            let reason = if phase_candidates.is_empty() {
                "no_phase_evidence"
            } else {
                "ambiguous_phase"
            };
            record.attribution.run_reason = Some(reason.into());
            record.attribution.phase_reason = Some(reason.into());
            record.attribution.round_reason = Some(
                if phase_candidates.is_empty() {
                    "no_round_evidence"
                } else {
                    "ambiguous_round"
                }
                .into(),
            );
        }
    }
}

fn init_db(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA foreign_keys=ON;
      CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);
      CREATE TABLE usage_record(record_id TEXT PRIMARY KEY,vendor TEXT NOT NULL,vendor_session_id TEXT NOT NULL,vendor_event_id TEXT NOT NULL,occurred_at TEXT,ordinal INTEGER,raw_model TEXT,source_basename TEXT NOT NULL,source_hash TEXT NOT NULL,collection_status TEXT NOT NULL,status_reason TEXT,input_uncached INTEGER,input_cache_write INTEGER,input_cache_read INTEGER,output INTEGER,output_reasoning INTEGER,vendor_usage_json TEXT NOT NULL);
      CREATE TABLE attribution(record_id TEXT PRIMARY KEY REFERENCES usage_record(record_id),aida_session_id TEXT,run_uuid TEXT,spec_id TEXT,seat TEXT,phase TEXT,round INTEGER,session_reason TEXT,run_reason TEXT,spec_reason TEXT,seat_reason TEXT,phase_reason TEXT,round_reason TEXT);
      CREATE TABLE coverage(key TEXT PRIMARY KEY,value INTEGER NOT NULL);
      CREATE INDEX attribution_spec ON attribution(spec_id);")?;
    Ok(())
}

pub(crate) fn rebuild(root: &Path, json: bool) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("resolve project {}", root.display()))?;
    let (parent, dest) = prepare_ledger_destination(&root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
    }
    let temp = parent.join(format!(".token-ledger-v1-{}.tmp", std::process::id()));
    if let Ok(metadata) = fs::symlink_metadata(&temp) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            bail!("refusing unsafe temporary ledger path: {}", temp.display());
        }
        fs::remove_file(&temp)?;
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let mut paths = Vec::new();
    if let Some(home) = crate::home_dir() {
        walk_jsonl(&home.join(".claude/projects"), &mut paths);
        walk_jsonl(&home.join(".codex/sessions"), &mut paths);
    }
    walk_jsonl(&root.join(".aida/headless-logs"), &mut paths);
    paths.sort();
    paths.dedup();
    let manifests = load_manifests(&root);
    let known_sessions: BTreeSet<_> = manifests
        .iter()
        .map(|m| m.vendor_session_id.as_str())
        .collect();
    let mut coverage = Coverage::default();
    let mut records = Vec::new();
    for path in paths {
        // Parse only project-local logs or global sessions named/referenced by an exact manifest.
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if !known_sessions.iter().any(|id| name.contains(*id)) {
            continue;
        }
        coverage.source_files += 1;
        let is_codex = path.to_string_lossy().contains("/.codex/")
            || file_contains_codex_marker(&path).unwrap_or(false);
        let parsed = if is_codex {
            parse_codex_file(&path, &mut coverage)
        } else {
            parse_claude_file(&path, &mut coverage)
        };
        match parsed {
            Ok(mut r) => records.append(&mut r),
            Err(_) => coverage.unsupported += 1,
        }
    }
    records.sort_by(|a, b| a.record_id.cmp(&b.record_id));
    records.dedup_by(|a, b| {
        if a.record_id == b.record_id {
            coverage.duplicates_suppressed += 1;
            true
        } else {
            false
        }
    });
    attribute(&mut records, &manifests, &load_phases(&root), &mut coverage);
    coverage.measured_unique = records
        .iter()
        .filter(|r| r.collection_status == "measured")
        .count();
    let mut conn = Connection::open(&temp)?;
    init_db(&conn)?;
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO metadata VALUES('schema_version',?1)",
        [SCHEMA_VERSION.to_string()],
    )?;
    tx.execute(
        "INSERT INTO metadata VALUES('collector_version',?1)",
        [env!("CARGO_PKG_VERSION")],
    )?;
    tx.execute(
        "INSERT INTO metadata VALUES('project_identity',?1)",
        [project_identity(&root)],
    )?;
    tx.execute(
        "INSERT INTO metadata VALUES('rebuilt_at',?1)",
        [Utc::now().to_rfc3339()],
    )?;
    let digest = sha(records
        .iter()
        .map(|r| r.record_id.as_str())
        .collect::<Vec<_>>()
        .join("\n")
        .as_bytes());
    tx.execute(
        "INSERT INTO metadata VALUES('input_inventory_digest',?1)",
        [digest],
    )?;
    for r in &records {
        tx.execute("INSERT INTO usage_record VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)", params![r.record_id,r.vendor,r.vendor_session_id,r.vendor_event_id,r.occurred_at,r.ordinal,r.raw_model,r.source_basename,r.source_hash,r.collection_status,r.status_reason,r.tokens.input_uncached,r.tokens.input_cache_write,r.tokens.input_cache_read,r.tokens.output,r.tokens.output_reasoning,serde_json::to_string(&r.vendor_usage)?])?;
        let a = &r.attribution;
        tx.execute(
            "INSERT INTO attribution VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                r.record_id,
                a.aida_session_id,
                a.run_uuid,
                a.spec_id,
                a.seat,
                a.phase,
                a.round,
                a.session_reason,
                a.run_reason,
                a.spec_reason,
                a.seat_reason,
                a.phase_reason,
                a.round_reason
            ],
        )?;
    }
    for (k, v) in [
        ("source_files", coverage.source_files),
        ("candidate_events", coverage.candidate_events),
        ("measured_unique", coverage.measured_unique),
        ("duplicates_suppressed", coverage.duplicates_suppressed),
        ("conflicts", coverage.conflicts),
        ("truncated", coverage.truncated),
        ("unsupported", coverage.unsupported),
        ("unattributed", coverage.unattributed),
    ] {
        tx.execute("INSERT INTO coverage VALUES(?1,?2)", params![k, v as i64])?;
    }
    tx.commit()?;
    conn.close().map_err(|(_, e)| e)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temp, &dest)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&coverage)?);
    } else {
        println!("rebuilt {}: {} measured records; {} duplicates; {} unattributed; {} unknown/unsupported", dest.display(), coverage.measured_unique, coverage.duplicates_suppressed, coverage.unattributed, coverage.truncated + coverage.unsupported + coverage.conflicts);
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct TokenTotal {
    total: Option<u64>,
    known_records: u64,
    missing_records: u64,
}

impl TokenTotal {
    fn display(&self) -> String {
        match self.total {
            Some(total) => format!(
                "{total} (known {}, missing {})",
                self.known_records, self.missing_records
            ),
            None => format!("unknown (known 0, missing {})", self.missing_records),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct Group {
    phase: Option<String>,
    vendor: Option<String>,
    model: Option<String>,
    round: Option<u32>,
    records: u64,
    input_uncached: TokenTotal,
    input_cache_write: TokenTotal,
    input_cache_read: TokenTotal,
    output: TokenTotal,
    output_reasoning: TokenTotal,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost: Option<GroupCost>,
}

// Cost is a derived estimate. Integer pico-USD prevents floating-point drift;
// the fixed six-decimal USD string is presentation only.
// trace:TASK-1434 | ai:codex
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
struct GroupCost {
    priced_measured_tokens: u64,
    unpriced_measured_tokens: u64,
    cost_pico_usd: i128,
    cost_usd: String,
    unpriced_reasons: BTreeMap<String, u64>,
    matched_rate_ids: BTreeSet<String>,
    resolved_models: BTreeSet<String>,
    alias_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct CostSummary {
    semantics: &'static str,
    currency: String,
    rate_table_version: String,
    rate_table_published_at: String,
    source_url: String,
    source_revision: String,
    source_retrieved_at: String,
    priced_measured_tokens: u64,
    unpriced_measured_tokens: u64,
    cost_pico_usd: i128,
    cost_usd: String,
    unpriced_reasons: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SpecCoverage {
    candidate_records: u64,
    measured_records: u64,
    non_measured_records: u64,
    records_with_unknown_dimensions: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct QueryPayload {
    schema_version: i64,
    spec_id: String,
    grouped_by_model: bool,
    groups: Vec<Group>,
    spec_coverage: SpecCoverage,
    attribution_reasons: BTreeMap<String, BTreeMap<String, u64>>,
    ledger_global_collector_diagnostics: BTreeMap<String, u64>,
    pricing: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost: Option<CostSummary>,
    legacy_drain_summary: &'static str,
}

fn reason_counts(conn: &Connection, spec: &str) -> Result<BTreeMap<String, BTreeMap<String, u64>>> {
    let mut out = BTreeMap::new();
    for (dimension, value, reason) in [
        ("run", "run_uuid", "run_reason"),
        ("spec", "spec_id", "spec_reason"),
        ("seat", "seat", "seat_reason"),
        ("phase", "phase", "phase_reason"),
        ("round", "round", "round_reason"),
    ] {
        let sql = format!(
            "SELECT COALESCE({reason},'missing_without_reason'),COUNT(*) FROM attribution WHERE upper(spec_id)=upper(?1) AND {value} IS NULL GROUP BY COALESCE({reason},'missing_without_reason') ORDER BY 1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let counts = stmt
            .query_map([spec], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<BTreeMap<String, u64>>>()?;
        out.insert(dimension.into(), counts);
    }
    Ok(out)
}

fn format_usd_6(pico: i128) -> String {
    // One micro-dollar is 1,000,000 pico-dollars. Round half-even only at the
    // presentation boundary; aggregation always uses exact pico-dollar ints.
    let divisor = 1_000_000i128;
    let quotient = pico / divisor;
    let remainder = pico % divisor;
    let half = divisor / 2;
    let micros = quotient
        + if remainder > half || (remainder == half && quotient % 2 != 0) {
            1
        } else {
            0
        };
    format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000)
}

fn add_reason(cost: &mut GroupCost, reason: &str, tokens: u64) -> Result<()> {
    cost.unpriced_measured_tokens = cost
        .unpriced_measured_tokens
        .checked_add(tokens)
        .ok_or_else(|| anyhow::anyhow!("token arithmetic overflow"))?;
    let count = cost.unpriced_reasons.entry(reason.to_string()).or_default();
    *count = count
        .checked_add(tokens)
        .ok_or_else(|| anyhow::anyhow!("token arithmetic overflow"))?;
    Ok(())
}

fn price_token_class(
    cost: &mut GroupCost,
    tokens: Option<u64>,
    rate: Option<&String>,
    class: &str,
) -> Result<()> {
    let Some(tokens) = tokens else {
        return Ok(());
    };
    let Some(rate) = rate else {
        add_reason(cost, &format!("missing_rate:{class}"), tokens)?;
        return Ok(());
    };
    let per_token = parse_rate_pico_per_token(rate)?;
    let line = per_token
        .checked_mul(i128::from(tokens))
        .ok_or_else(|| anyhow::anyhow!("cost arithmetic overflow"))?;
    cost.cost_pico_usd = cost
        .cost_pico_usd
        .checked_add(line)
        .ok_or_else(|| anyhow::anyhow!("cost arithmetic overflow"))?;
    cost.priced_measured_tokens = cost
        .priced_measured_tokens
        .checked_add(tokens)
        .ok_or_else(|| anyhow::anyhow!("token arithmetic overflow"))?;
    Ok(())
}

fn apply_pricing(
    conn: &Connection,
    spec: &str,
    dims: &BTreeSet<&str>,
    payload: &mut QueryPayload,
) -> Result<()> {
    let table = load_rate_table()?;
    let mut stmt = conn.prepare("SELECT a.phase,u.vendor,u.raw_model,a.round,u.occurred_at,u.input_uncached,u.input_cache_write,u.input_cache_read,u.output,u.output_reasoning FROM usage_record u JOIN attribution a USING(record_id) WHERE upper(a.spec_id)=upper(?1) AND u.collection_status='measured' ORDER BY u.record_id")?;
    let rows = stmt.query_map([spec], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<u32>>(3)?,
            r.get::<_, Option<String>>(4)?,
            [r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?],
        ))
    })?;
    for row in rows {
        let (phase, vendor, model, round, occurred_at, tokens): (
            Option<String>,
            String,
            Option<String>,
            Option<u32>,
            Option<String>,
            [Option<u64>; 5],
        ) = row?;
        let group = payload
            .groups
            .iter_mut()
            .find(|g| {
                (!dims.contains("phase") || g.phase == phase)
                    && (!dims.contains("vendor") || g.vendor.as_deref() == Some(vendor.as_str()))
                    && (!dims.contains("model") || g.model == model)
                    && (!dims.contains("round") || g.round == round)
            })
            .ok_or_else(|| anyhow::anyhow!("priced row did not map to a usage group"))?;
        let cost = group.cost.get_or_insert_with(GroupCost::default);
        let known_total = tokens
            .iter()
            .flatten()
            .try_fold(0u64, |sum, value| sum.checked_add(*value))
            .ok_or_else(|| anyhow::anyhow!("token arithmetic overflow"))?;
        let Some(model) = model.as_deref() else {
            add_reason(cost, "missing_model", known_total)?;
            continue;
        };
        let Some(occurred_at) = occurred_at.as_deref() else {
            add_reason(cost, "missing_timestamp", known_total)?;
            continue;
        };
        let at = match parse_rate_time(occurred_at) {
            Ok(at) => at,
            Err(_) => {
                add_reason(cost, "invalid_timestamp", known_total)?;
                continue;
            }
        };
        let matched = match match_rate(&table, &vendor, model, at) {
            Ok(matched) => matched,
            Err(reason) => {
                add_reason(cost, &reason, known_total)?;
                continue;
            }
        };
        cost.matched_rate_ids.insert(matched.entry.id.clone());
        cost.resolved_models
            .insert(matched.resolved_model.to_string());
        if let Some(alias) = matched.alias_id {
            cost.alias_ids.insert(alias.to_string());
        }
        price_token_class(
            cost,
            tokens[0],
            matched.entry.input_uncached.as_ref(),
            "input_uncached",
        )?;
        price_token_class(
            cost,
            tokens[1],
            matched.entry.input_cache_write.as_ref(),
            "input_cache_write",
        )?;
        price_token_class(
            cost,
            tokens[2],
            matched.entry.input_cache_read.as_ref(),
            "input_cache_read",
        )?;
        price_token_class(cost, tokens[3], matched.entry.output.as_ref(), "output")?;
        price_token_class(
            cost,
            tokens[4],
            matched.entry.output_reasoning.as_ref(),
            "output_reasoning",
        )?;
    }
    let mut summary = CostSummary {
        semantics: "local_estimate_not_billing",
        currency: table.currency.clone(),
        rate_table_version: table.table_version.clone(),
        rate_table_published_at: table.published_at.clone(),
        source_url: table.source_url.clone(),
        source_revision: table.source_revision.clone(),
        source_retrieved_at: table.source_retrieved_at.clone(),
        priced_measured_tokens: 0,
        unpriced_measured_tokens: 0,
        cost_pico_usd: 0,
        cost_usd: String::new(),
        unpriced_reasons: BTreeMap::new(),
    };
    for group in &mut payload.groups {
        if let Some(cost) = &mut group.cost {
            cost.cost_usd = format_usd_6(cost.cost_pico_usd);
            summary.priced_measured_tokens = summary
                .priced_measured_tokens
                .checked_add(cost.priced_measured_tokens)
                .ok_or_else(|| anyhow::anyhow!("token arithmetic overflow"))?;
            summary.unpriced_measured_tokens = summary
                .unpriced_measured_tokens
                .checked_add(cost.unpriced_measured_tokens)
                .ok_or_else(|| anyhow::anyhow!("token arithmetic overflow"))?;
            summary.cost_pico_usd = summary
                .cost_pico_usd
                .checked_add(cost.cost_pico_usd)
                .ok_or_else(|| anyhow::anyhow!("cost arithmetic overflow"))?;
            for (reason, count) in &cost.unpriced_reasons {
                let total = summary.unpriced_reasons.entry(reason.clone()).or_default();
                *total = total
                    .checked_add(*count)
                    .ok_or_else(|| anyhow::anyhow!("token arithmetic overflow"))?;
            }
        }
    }
    summary.cost_usd = format_usd_6(summary.cost_pico_usd);
    payload.cost = Some(summary);
    Ok(())
}

fn query_payload(
    conn: &Connection,
    spec: &str,
    group_by: &str,
    with_cost: bool,
) -> Result<QueryPayload> {
    let dims: BTreeSet<_> = group_by.split(',').map(str::trim).collect();
    if dims
        .iter()
        .any(|d| !matches!(*d, "phase" | "vendor" | "model" | "round"))
    {
        bail!("--group-by accepts phase,vendor,model,round");
    }
    let phase_expr = if dims.contains("phase") {
        "a.phase"
    } else {
        "NULL"
    };
    let vendor_expr = if dims.contains("vendor") {
        "u.vendor"
    } else {
        "NULL"
    };
    let model_expr = if dims.contains("model") {
        "u.raw_model"
    } else {
        "NULL"
    };
    let round_expr = if dims.contains("round") {
        "a.round"
    } else {
        "NULL"
    };
    let query = format!(
        "SELECT {phase_expr},{vendor_expr},{model_expr},{round_expr},COUNT(*),SUM(u.input_uncached),COUNT(u.input_uncached),SUM(u.input_cache_write),COUNT(u.input_cache_write),SUM(u.input_cache_read),COUNT(u.input_cache_read),SUM(u.output),COUNT(u.output),SUM(u.output_reasoning),COUNT(u.output_reasoning) FROM usage_record u JOIN attribution a USING(record_id) WHERE upper(a.spec_id)=upper(?1) AND u.collection_status='measured' GROUP BY {phase_expr},{vendor_expr},{model_expr},{round_expr} ORDER BY {phase_expr},{vendor_expr},{model_expr},{round_expr}"
    );
    let mut stmt = conn.prepare(&query)?;
    let groups = stmt
        .query_map([spec], |r| {
            let records: u64 = r.get(4)?;
            let total = |sum_index, count_index| -> rusqlite::Result<TokenTotal> {
                let known: u64 = r.get(count_index)?;
                Ok(TokenTotal {
                    total: r.get(sum_index)?,
                    known_records: known,
                    missing_records: records - known,
                })
            };
            Ok(Group {
                phase: if dims.contains("phase") {
                    r.get(0)?
                } else {
                    None
                },
                vendor: if dims.contains("vendor") {
                    r.get(1)?
                } else {
                    None
                },
                model: if dims.contains("model") {
                    r.get(2)?
                } else {
                    None
                },
                round: if dims.contains("round") {
                    r.get(3)?
                } else {
                    None
                },
                records,
                input_uncached: total(5, 6)?,
                input_cache_write: total(7, 8)?,
                input_cache_read: total(9, 10)?,
                output: total(11, 12)?,
                output_reasoning: total(13, 14)?,
                cost: None,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let ledger_global_collector_diagnostics = {
        let mut statement = conn.prepare("SELECT key,value FROM coverage ORDER BY key")?;
        let values = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<BTreeMap<String, u64>>>()?;
        values
    };
    let (candidate_records, measured_records, unknown): (u64, u64, u64) = conn.query_row(
        "SELECT COUNT(*),SUM(CASE WHEN u.collection_status='measured' THEN 1 ELSE 0 END),SUM(CASE WHEN a.run_uuid IS NULL OR a.spec_id IS NULL OR a.seat IS NULL OR a.phase IS NULL OR a.round IS NULL THEN 1 ELSE 0 END) FROM usage_record u JOIN attribution a USING(record_id) WHERE upper(a.spec_id)=upper(?1)",
        [spec], |r| Ok((r.get(0)?, r.get::<_, Option<u64>>(1)?.unwrap_or(0), r.get::<_, Option<u64>>(2)?.unwrap_or(0))))?;
    let mut payload = QueryPayload {
        schema_version: SCHEMA_VERSION,
        spec_id: spec.to_ascii_uppercase(),
        grouped_by_model: dims.contains("model"),
        groups,
        spec_coverage: SpecCoverage {
            candidate_records,
            measured_records,
            non_measured_records: candidate_records - measured_records,
            records_with_unknown_dimensions: unknown,
        },
        attribution_reasons: reason_counts(conn, spec)?,
        ledger_global_collector_diagnostics,
        pricing: if with_cost {
            "local_estimate_not_billing"
        } else {
            "not_requested"
        },
        cost: None,
        legacy_drain_summary: "lower_bound_only",
    };
    if with_cost {
        apply_pricing(conn, spec, &dims, &mut payload)?;
    }
    Ok(payload)
}

fn render_query(payload: &QueryPayload, json: bool, toon: bool) -> Result<String> {
    if json {
        return Ok(serde_json::to_string_pretty(payload)?);
    }
    let mut out = String::new();
    if toon {
        out.push_str(&format!("spec_id: {}\nspec_coverage: candidates={},measured={},non_measured={},unknown_dimensions={}\n", payload.spec_id, payload.spec_coverage.candidate_records, payload.spec_coverage.measured_records, payload.spec_coverage.non_measured_records, payload.spec_coverage.records_with_unknown_dimensions));
    } else {
        out.push_str(&format!("Token usage for {} (local telemetry; not billing)\nSpec coverage: candidates={} measured={} non-measured={} unknown-dimensions={}\n", payload.spec_id, payload.spec_coverage.candidate_records, payload.spec_coverage.measured_records, payload.spec_coverage.non_measured_records, payload.spec_coverage.records_with_unknown_dimensions));
    }
    for g in &payload.groups {
        let model = if payload.grouped_by_model {
            format!(" model={}", g.model.as_deref().unwrap_or("unknown"))
        } else {
            String::new()
        };
        out.push_str(&format!("  phase={} vendor={}{} round={} records={} uncached={} cache-write={} cache-read={} output={} reasoning={}\n", g.phase.as_deref().unwrap_or("unknown"), g.vendor.as_deref().unwrap_or("unknown"), model, g.round.map(|x|x.to_string()).unwrap_or_else(||"unknown".into()), g.records, g.input_uncached.display(), g.input_cache_write.display(), g.input_cache_read.display(), g.output.display(), g.output_reasoning.display()));
        if let Some(cost) = &g.cost {
            out.push_str(&format!(
                "    cost_usd={} priced_tokens={} unpriced_tokens={} unpriced_reasons={:?}\n",
                cost.cost_usd,
                cost.priced_measured_tokens,
                cost.unpriced_measured_tokens,
                cost.unpriced_reasons
            ));
        }
    }
    if payload.groups.is_empty() {
        out.push_str("  no measured attributed records (unknown, not zero)\n");
    }
    out.push_str("Attribution reasons (spec-scoped):\n");
    for (dimension, reasons) in &payload.attribution_reasons {
        out.push_str(&format!(
            "  {dimension}: {}\n",
            if reasons.is_empty() {
                "none".into()
            } else {
                reasons
                    .iter()
                    .map(|(r, n)| format!("{r}={n}"))
                    .collect::<Vec<_>>()
                    .join(",")
            }
        ));
    }
    out.push_str(&format!(
        "Ledger-global collector diagnostics (not spec coverage): {:?}\n",
        payload.ledger_global_collector_diagnostics
    ));
    if let Some(cost) = &payload.cost {
        out.push_str(&format!("Cost estimate: USD {} ({} pico-USD); priced measured tokens={} unpriced measured tokens={} reasons={:?}\nRate table {} published {} source revision {}\nLOCAL ESTIMATE ONLY — NOT BILLING; historical drain summaries are lower bounds only", cost.cost_usd, cost.cost_pico_usd, cost.priced_measured_tokens, cost.unpriced_measured_tokens, cost.unpriced_reasons, cost.rate_table_version, cost.rate_table_published_at, cost.source_revision));
    } else {
        out.push_str(
            "pricing: not requested; use --cost; historical drain summaries are lower bounds only",
        );
    }
    Ok(out)
}

pub(crate) fn show(
    root: &Path,
    spec: &str,
    group_by: &str,
    cost: bool,
    json: bool,
    toon: bool,
) -> Result<()> {
    let path = ledger_path(root);
    if !path.exists() {
        bail!("token ledger not found; run `aida usage rebuild`");
    }
    let conn = Connection::open(path)?;
    let payload = query_payload(&conn, spec, group_by, cost)?;
    println!("{}", render_query(&payload, json, toon)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fixture_rate_table() -> RateTable {
        RateTable {
            schema_version: 1,
            table_version: "test-v1".into(),
            published_at: "2026-01-01T00:00:00Z".into(),
            currency: "USD".into(),
            unit_tokens: 1_000_000,
            source_url: "https://example.test/rates".into(),
            source_revision: "abc".into(),
            source_retrieved_at: "2026-01-01T00:00:00Z".into(),
            license: "test".into(),
            rates: vec![RateEntry {
                id: "r1".into(),
                provider: "claude".into(),
                model: "exact-model".into(),
                effective_from: "2026-01-01T00:00:00Z".into(),
                effective_to: Some("2026-02-01T00:00:00Z".into()),
                currency: "USD".into(),
                source_url: "https://example.test/rates".into(),
                source_revision: "abc".into(),
                source_retrieved_at: "2026-01-01T00:00:00Z".into(),
                input_uncached: Some("3.000000".into()),
                input_cache_write: Some("3.750000".into()),
                input_cache_read: Some("0.300000".into()),
                output: Some("15.000000".into()),
                output_reasoning: None,
            }],
            aliases: vec![RateAlias {
                id: "a1".into(),
                provider: "claude".into(),
                alias: "friendly-model".into(),
                canonical_model: "exact-model".into(),
                effective_from: "2026-01-01T00:00:00Z".into(),
                effective_to: Some("2026-02-01T00:00:00Z".into()),
                source_url: "https://example.test/rates".into(),
                source_revision: "abc".into(),
                source_retrieved_at: "2026-01-01T00:00:00Z".into(),
            }],
        }
    }

    // trace:TASK-1434 | ai:codex
    #[test]
    fn rate_matching_is_exact_dated_and_aliases_are_explicit() {
        let table = fixture_rate_table();
        validate_rate_table(&table).unwrap();
        let start = parse_rate_time("2026-01-01T00:00:00Z").unwrap();
        let end = parse_rate_time("2026-02-01T00:00:00Z").unwrap();
        assert_eq!(
            match_rate(&table, "claude", "exact-model", start)
                .unwrap()
                .entry
                .id,
            "r1"
        );
        let alias = match_rate(&table, "claude", "friendly-model", start).unwrap();
        assert_eq!(alias.alias_id, Some("a1"));
        assert_eq!(alias.resolved_model, "exact-model");
        assert_eq!(
            match_rate(&table, "claude", "exact", start).unwrap_err(),
            "unknown_model"
        );
        assert_eq!(
            match_rate(&table, "claude", "exact-model", end).unwrap_err(),
            "rate_out_of_range"
        );
        let mut ambiguous = table.clone();
        let mut duplicate = ambiguous.rates[0].clone();
        duplicate.id = "r2".into();
        ambiguous.rates.push(duplicate);
        assert!(validate_rate_table(&ambiguous)
            .unwrap_err()
            .to_string()
            .contains("overlapping rate intervals"));
    }

    #[test]
    fn fixed_point_pricing_keeps_zero_missing_classes_and_rounding_distinct() {
        assert_eq!(parse_rate_pico_per_token("3.000000").unwrap(), 3_000_000);
        assert!(parse_rate_pico_per_token("0.0000001").is_err());
        let mut cost = GroupCost::default();
        price_token_class(&mut cost, Some(0), Some(&"3.000000".into()), "input").unwrap();
        price_token_class(&mut cost, Some(2), None, "reasoning").unwrap();
        assert_eq!(cost.priced_measured_tokens, 0);
        assert_eq!(cost.cost_pico_usd, 0);
        assert_eq!(cost.unpriced_measured_tokens, 2);
        assert_eq!(cost.unpriced_reasons["missing_rate:reasoning"], 2);
        assert_eq!(format_usd_6(1_500_000), "0.000002");
        assert_eq!(format_usd_6(2_500_000), "0.000002", "half-even");
    }

    #[test]
    fn cost_query_groups_implementation_review_rework_and_mixed_coverage() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE usage_record(record_id TEXT PRIMARY KEY,vendor TEXT NOT NULL,vendor_session_id TEXT NOT NULL,vendor_event_id TEXT NOT NULL,occurred_at TEXT,ordinal INTEGER,raw_model TEXT,source_basename TEXT NOT NULL,source_hash TEXT NOT NULL,collection_status TEXT NOT NULL,status_reason TEXT,input_uncached INTEGER,input_cache_write INTEGER,input_cache_read INTEGER,output INTEGER,output_reasoning INTEGER,vendor_usage_json TEXT NOT NULL);
             CREATE TABLE attribution(record_id TEXT PRIMARY KEY,aida_session_id TEXT,run_uuid TEXT,spec_id TEXT,seat TEXT,phase TEXT,round INTEGER,session_reason TEXT,run_reason TEXT,spec_reason TEXT,seat_reason TEXT,phase_reason TEXT,round_reason TEXT);
             CREATE TABLE coverage(key TEXT PRIMARY KEY,value INTEGER NOT NULL);
             INSERT INTO coverage VALUES('measured_unique',3);
             INSERT INTO usage_record VALUES('i','claude','s','i','2026-01-01T00:00:00Z',1,'exact-model','f','h','measured',NULL,1,NULL,NULL,NULL,2,'{}');
             INSERT INTO attribution VALUES('i','a','run','TASK-1434','implementer','implementer',1,NULL,NULL,NULL,NULL,NULL,NULL);
             INSERT INTO usage_record VALUES('v','claude','s','v','2026-01-01T00:00:01Z',2,'friendly-model','f','h','measured',NULL,0,NULL,NULL,2,NULL,'{}');
             INSERT INTO attribution VALUES('v','a','run','TASK-1434','reviewer','reviewer',1,NULL,NULL,NULL,NULL,NULL,NULL);
             INSERT INTO usage_record VALUES('r','claude','s','r','2026-01-01T00:00:02Z',3,'unknown-model','f','h','measured',NULL,3,NULL,NULL,NULL,NULL,'{}');
             INSERT INTO attribution VALUES('r','a','run','TASK-1434','implementer','rework',2,NULL,NULL,NULL,NULL,NULL,NULL);",
        )
        .unwrap();
        let mut table = fixture_rate_table();
        // Exercise the production data shape by temporarily checking the same
        // arithmetic against its independently parsed fixture first.
        table.rates[0].effective_to = Some("2027-01-01T00:00:00Z".into());
        table.aliases[0].effective_to = Some("2027-01-01T00:00:00Z".into());
        assert!(match_rate(
            &table,
            "claude",
            "friendly-model",
            parse_rate_time("2026-01-01T00:00:01Z").unwrap()
        )
        .is_ok());

        // The bundled table uses different production model names, so replace
        // fixture identifiers with its explicit alias/direct model names.
        conn.execute(
            "UPDATE usage_record SET raw_model='claude-sonnet-4-20250514' WHERE record_id='i'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE usage_record SET raw_model='claude-sonnet-4' WHERE record_id='v'",
            [],
        )
        .unwrap();
        let payload = query_payload(&conn, "TASK-1434", "phase,vendor,model,round", true).unwrap();
        assert_eq!(payload.groups.len(), 3);
        assert_eq!(
            payload
                .groups
                .iter()
                .map(|g| g.phase.as_deref().unwrap())
                .collect::<Vec<_>>(),
            vec!["implementer", "reviewer", "rework"]
        );
        let cost = payload.cost.as_ref().unwrap();
        assert_eq!(
            cost.priced_measured_tokens, 3,
            "zero is known but adds no tokens"
        );
        assert_eq!(cost.unpriced_measured_tokens, 5);
        assert_eq!(cost.cost_pico_usd, 33_000_000);
        assert_eq!(cost.cost_usd, "0.000033");
        assert_eq!(cost.unpriced_reasons["missing_rate:output_reasoning"], 2);
        assert_eq!(cost.unpriced_reasons["unknown_model"], 3);
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["pricing"], "local_estimate_not_billing");
        let human = render_query(&payload, false, false).unwrap();
        let toon = render_query(&payload, false, true).unwrap();
        assert!(human.contains("LOCAL ESTIMATE ONLY — NOT BILLING"));
        assert!(toon.contains("Rate table 2026-09-22.1"));
    }

    #[test]
    fn claude_duplicate_representation_is_counted_once() {
        let d = tempdir().unwrap();
        let p = d.path().join("c.jsonl");
        let line = r#"{"session_id":"s","uuid":"u","requestId":"q","timestamp":"2026-01-01T00:00:00Z","message":{"id":"m","model":"claude-x","usage":{"input_tokens":2,"cache_creation_input_tokens":3,"cache_read_input_tokens":4,"output_tokens":5,"output_tokens_details":{"thinking_tokens":1}}}}"#;
        fs::write(&p, format!("{line}\n{line}\n")).unwrap();
        let mut c = Coverage::default();
        let r = parse_claude_file(&p, &mut c).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(c.duplicates_suppressed, 1);
        assert_eq!(r[0].tokens.input_cache_read, Some(4));
        assert_eq!(r[0].tokens.output_reasoning, Some(1));
    }

    #[test]
    fn codex_repeated_cumulative_checkpoint_is_counted_once() {
        let d = tempdir().unwrap();
        let p = d.path().join("s.jsonl");
        let line = r#"{"timestamp":"2026-01-01T00:00:00Z","ordinal":1,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":4,"output_tokens":2,"total_tokens":12},"last_token_usage":{"input_tokens":10,"cached_input_tokens":4,"cache_write_input_tokens":1,"output_tokens":2,"reasoning_output_tokens":1,"total_tokens":12}}}}"#;
        fs::write(&p, format!("{line}\n{line}\n")).unwrap();
        let mut c = Coverage::default();
        let r = parse_codex_file(&p, &mut c).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(c.duplicates_suppressed, 1);
        assert_eq!(r[0].tokens.input_uncached, Some(6));
    }

    #[test]
    fn exact_session_join_distinguishes_missing_and_ambiguous() {
        let mut records = vec![Record {
            record_id: "r".into(),
            vendor: "claude".into(),
            vendor_session_id: "s".into(),
            vendor_event_id: "e".into(),
            occurred_at: Some("2026-01-01T00:00:01Z".into()),
            ordinal: None,
            raw_model: None,
            source_basename: "x".into(),
            source_hash: "h".into(),
            collection_status: "measured".into(),
            status_reason: None,
            tokens: Tokens::default(),
            vendor_usage: Value::Null,
            attribution: Attribution::default(),
        }];
        let item = ManifestItem {
            spec_id: "TASK-1".into(),
            started_at: None,
            completed_at: None,
        };
        let m = ManifestEvidence {
            aida_session_id: "a".into(),
            vendor_session_id: "s".into(),
            items: vec![item],
            role: Some("implementer".into()),
        };
        let mut c = Coverage::default();
        attribute(&mut records, &[m.clone(), m], &[], &mut c);
        assert_eq!(
            records[0].attribution.session_reason.as_deref(),
            Some("multiple_candidates")
        );
        assert!(records[0].attribution.spec_id.is_none());
    }

    #[test]
    fn implementation_review_rework_fixture_keeps_phase_round_and_vendor() {
        let base = "2026-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let manifests = vec![ManifestEvidence {
            aida_session_id: "aida-1".into(),
            vendor_session_id: "vendor-1".into(),
            items: vec![ManifestItem {
                spec_id: "TASK-1427".into(),
                started_at: Some(base),
                completed_at: None,
            }],
            role: Some("implementer".into()),
        }];
        let phases = vec![
            PhaseInterval {
                spec: "TASK-1427".into(),
                run_uuid: "run-1".into(),
                phase: "implementer".into(),
                round: Some(1),
                round_reason: None,
                start: base,
                end: Some(base + chrono::Duration::seconds(10)),
            },
            PhaseInterval {
                spec: "TASK-1427".into(),
                run_uuid: "run-1".into(),
                phase: "reviewer".into(),
                round: Some(1),
                round_reason: None,
                start: base + chrono::Duration::seconds(10),
                end: Some(base + chrono::Duration::seconds(20)),
            },
            PhaseInterval {
                spec: "TASK-1427".into(),
                run_uuid: "run-1".into(),
                phase: "implementer".into(),
                round: Some(2),
                round_reason: None,
                start: base + chrono::Duration::seconds(20),
                end: None,
            },
        ];
        let mut records: Vec<Record> = [5, 15, 25]
            .into_iter()
            .enumerate()
            .map(|(idx, second)| Record {
                record_id: format!("r{idx}"),
                vendor: if idx == 1 { "codex" } else { "claude" }.into(),
                vendor_session_id: "vendor-1".into(),
                vendor_event_id: format!("e{idx}"),
                occurred_at: Some((base + chrono::Duration::seconds(second)).to_rfc3339()),
                ordinal: Some(idx as i64),
                raw_model: None,
                source_basename: "fixture.jsonl".into(),
                source_hash: "hash".into(),
                collection_status: "measured".into(),
                status_reason: None,
                tokens: Tokens {
                    input_uncached: Some(1),
                    ..Tokens::default()
                },
                vendor_usage: Value::Null,
                attribution: Attribution::default(),
            })
            .collect();
        let mut coverage = Coverage::default();
        attribute(&mut records, &manifests, &phases, &mut coverage);
        assert_eq!(
            records
                .iter()
                .map(|r| (r.attribution.phase.as_deref(), r.attribution.round))
                .collect::<Vec<_>>(),
            vec![
                (Some("implementer"), Some(1)),
                (Some("reviewer"), Some(1)),
                (Some("implementer"), Some(2))
            ]
        );
        assert_eq!(coverage.unattributed, 0);
    }

    #[test]
    fn truncated_rows_are_unknown_not_measured_zero() {
        let d = tempdir().unwrap();
        let p = d.path().join("bad.jsonl");
        fs::write(&p, "{broken\n").unwrap();
        let mut coverage = Coverage::default();
        let records = parse_claude_file(&p, &mut coverage).unwrap();
        assert!(records.is_empty());
        assert_eq!(coverage.truncated, 1);
        assert_eq!(coverage.measured_unique, 0);
    }

    #[test]
    fn claude_conflicting_duplicate_is_excluded() {
        let d = tempdir().unwrap();
        let p = d.path().join("c.jsonl");
        let a =
            r#"{"session_id":"s","requestId":"q","message":{"id":"m","usage":{"input_tokens":1}}}"#;
        let b =
            r#"{"session_id":"s","requestId":"q","message":{"id":"m","usage":{"input_tokens":2}}}"#;
        fs::write(&p, format!("{a}\n{b}\n")).unwrap();
        let mut coverage = Coverage::default();
        let records = parse_claude_file(&p, &mut coverage).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].collection_status, "conflict");
        assert_eq!(records[0].tokens, Tokens::default());
        assert_eq!(coverage.conflicts, 1);
    }

    #[test]
    fn codex_response_id_is_identity_and_conflicts_fail_closed() {
        let d = tempdir().unwrap();
        let p = d.path().join("codex.jsonl");
        let row = |response: &str, input: u64| {
            format!(
                r#"{{"timestamp":"2026-01-01T00:00:00Z","ordinal":1,"type":"token_usage_record","payload":{{"thread_id":"thread","response_id":"{response}","usage":{{"input_tokens":{input},"cached_input_tokens":0,"output_tokens":0}}}}}}"#
            )
        };
        fs::write(
            &p,
            format!("{}\n{}\n{}\n", row("r1", 1), row("r2", 1), row("r1", 2)),
        )
        .unwrap();
        let mut coverage = Coverage::default();
        let records = parse_codex_file(&p, &mut coverage).unwrap();
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|r| r.vendor_event_id == "r2"));
        let conflict = records.iter().find(|r| r.vendor_event_id == "r1").unwrap();
        assert_eq!(conflict.collection_status, "conflict");
        assert_eq!(coverage.conflicts, 1);
    }

    #[test]
    fn codex_mixed_stable_and_legacy_reconciles_only_matching_request() {
        let d = tempdir().unwrap();
        let p = d.path().join("codex.jsonl");
        let stable = serde_json::json!({"timestamp":"2026-01-01T00:00:01Z","ordinal":1,"type":"token_usage_record","payload":{"session_id":"session","response_id":"response-1","usage":{"input_tokens":10,"cached_input_tokens":0,"output_tokens":2,"total_tokens":12}}});
        let matching = serde_json::json!({"timestamp":"2026-01-01T00:00:01Z","ordinal":1,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":12},"last_token_usage":{"input_tokens":10,"cached_input_tokens":0,"output_tokens":2,"total_tokens":12}}}});
        let distinct = serde_json::json!({"timestamp":"2026-01-01T00:00:02Z","ordinal":2,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":19},"last_token_usage":{"input_tokens":5,"cached_input_tokens":0,"output_tokens":2,"total_tokens":7}}}});
        let meta = serde_json::json!({"type":"session_meta","payload":{"id":"session"}});
        fs::write(&p, format!("{meta}\n{stable}\n{matching}\n{distinct}\n")).unwrap();
        let mut coverage = Coverage::default();
        let records = parse_codex_file(&p, &mut coverage).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(coverage.duplicates_suppressed, 1);
        assert!(records.iter().any(|r| r.vendor_event_id == "response-1"));
        let legacy = records
            .iter()
            .find(|r| r.vendor_event_id.contains("ordinal-2"))
            .unwrap();
        assert_eq!(
            legacy.status_reason.as_deref(),
            Some("unmatched_legacy_fallback")
        );
        assert_eq!(legacy.tokens.input_uncached, Some(5));
    }

    #[test]
    fn codex_cumulative_reset_starts_new_segment_without_hash_identity() {
        let d = tempdir().unwrap();
        let p = d.path().join("codex.jsonl");
        let row = |ordinal: u64, total: u64| {
            serde_json::json!({"ordinal":ordinal,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":total},"last_token_usage":{"input_tokens":total,"output_tokens":0,"total_tokens":total}}}}).to_string()
        };
        fs::write(
            &p,
            format!("{}\n{}\n{}\n", row(1, 10), row(2, 5), row(3, 10)),
        )
        .unwrap();
        let mut coverage = Coverage::default();
        let records = parse_codex_file(&p, &mut coverage).unwrap();
        assert_eq!(records.len(), 3);
        assert!(records
            .iter()
            .all(|r| r.vendor_event_id.contains("ordinal-")));
        assert!(records
            .iter()
            .any(|r| r.vendor_event_id.starts_with("checkpoint:1:")));
    }

    #[test]
    fn unsupported_identity_is_counted_not_zeroed() {
        let d = tempdir().unwrap();
        let p = d.path().join("codex.jsonl");
        fs::write(
            &p,
            r#"{"type":"token_usage_record","payload":{"usage":{"input_tokens":0}}}"#,
        )
        .unwrap();
        let mut coverage = Coverage::default();
        let records = parse_codex_file(&p, &mut coverage).unwrap();
        assert!(records.is_empty());
        assert_eq!(coverage.unsupported, 1);
    }

    #[test]
    fn missing_session_and_spec_fill_every_downstream_reason() {
        let mut records = vec![Record {
            record_id: "r".into(),
            vendor: "claude".into(),
            vendor_session_id: "none".into(),
            vendor_event_id: "e".into(),
            occurred_at: None,
            ordinal: None,
            raw_model: None,
            source_basename: "x".into(),
            source_hash: "h".into(),
            collection_status: "measured".into(),
            status_reason: None,
            tokens: Tokens::default(),
            vendor_usage: Value::Null,
            attribution: Attribution::default(),
        }];
        let mut coverage = Coverage::default();
        attribute(&mut records, &[], &[], &mut coverage);
        let a = &records[0].attribution;
        assert!(
            a.session_reason.is_some()
                && a.run_reason.is_some()
                && a.spec_reason.is_some()
                && a.seat_reason.is_some()
                && a.phase_reason.is_some()
                && a.round_reason.is_some()
        );
        let manifest = ManifestEvidence {
            aida_session_id: "a".into(),
            vendor_session_id: "none".into(),
            items: vec![],
            role: None,
        };
        attribute(&mut records, &[manifest], &[], &mut coverage);
        let a = &records[0].attribution;
        assert_eq!(a.seat_reason.as_deref(), Some("no_lease_role"));
        assert!(
            a.run_reason.is_some()
                && a.spec_reason.is_some()
                && a.phase_reason.is_some()
                && a.round_reason.is_some()
        );
    }

    #[test]
    fn nullable_token_total_distinguishes_zero_from_unknown() {
        let zero = TokenTotal {
            total: Some(0),
            known_records: 1,
            missing_records: 0,
        };
        let unknown = TokenTotal {
            total: None,
            known_records: 0,
            missing_records: 1,
        };
        assert_ne!(zero.display(), unknown.display());
        assert_eq!(serde_json::to_value(&zero).unwrap()["total"], 0);
        assert!(serde_json::to_value(&unknown).unwrap()["total"].is_null());
    }

    #[test]
    fn rebuild_is_idempotent_and_owner_only() {
        let d = tempdir().unwrap();
        let root = d.path();
        fs::create_dir_all(root.join(".aida/sessions")).unwrap();
        fs::create_dir_all(root.join(".aida/headless-logs")).unwrap();
        fs::write(root.join(".aida/sessions/a.manifest.toml"),"session_id = \"a\"\nplanned_at = 2026-01-01T00:00:00Z\nplan_source = \"test\"\nclaude_session_id = \"fixture-session\"\n[[items]]\nspec_id = \"TASK-1\"\nposition = 1\nstatus_at_plan = \"In Progress\"\n").unwrap();
        fs::write(
            root.join(".aida/sessions/a.toml"),
            "id = \"a\"\nrole = \"implementer\"\nscope = \"TASK-1\"\n",
        )
        .unwrap();
        fs::write(root.join(".aida/headless-logs/x-fixture-session.jsonl"),r#"{"session_id":"fixture-session","uuid":"u","requestId":"q","message":{"id":"m","usage":{"input_tokens":0}}}"#).unwrap();
        rebuild(root, false).unwrap();
        let first = Connection::open(ledger_path(root)).unwrap();
        let logical_rows = |conn: &Connection| {
            let mut statement = conn.prepare("SELECT quote(u.record_id)||'|'||quote(u.vendor)||'|'||quote(u.vendor_session_id)||'|'||quote(u.vendor_event_id)||'|'||quote(u.occurred_at)||'|'||quote(u.ordinal)||'|'||quote(u.collection_status)||'|'||quote(u.status_reason)||'|'||quote(u.input_uncached)||'|'||quote(u.input_cache_write)||'|'||quote(u.input_cache_read)||'|'||quote(u.output)||'|'||quote(u.output_reasoning)||'|'||quote(u.vendor_usage_json)||'|'||quote(a.aida_session_id)||'|'||quote(a.run_uuid)||'|'||quote(a.spec_id)||'|'||quote(a.seat)||'|'||quote(a.phase)||'|'||quote(a.round)||'|'||quote(a.session_reason)||'|'||quote(a.run_reason)||'|'||quote(a.spec_reason)||'|'||quote(a.seat_reason)||'|'||quote(a.phase_reason)||'|'||quote(a.round_reason) FROM usage_record u JOIN attribution a USING(record_id) ORDER BY u.record_id").unwrap();
            let values = statement
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            values
        };
        let first_rows = logical_rows(&first);
        let first_totals = query_payload(&first, "TASK-1", "phase,vendor,round", false).unwrap();
        drop(first);
        rebuild(root, false).unwrap();
        let second = Connection::open(ledger_path(root)).unwrap();
        let second_rows = logical_rows(&second);
        let second_totals = query_payload(&second, "TASK-1", "phase,vendor,round", false).unwrap();
        assert_eq!(first_rows, second_rows);
        assert_eq!(first_totals, second_totals);
        assert_eq!(second_rows.len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(ledger_path(root))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn query_golden_covers_lifecycle_unknowns_and_all_formats() {
        let d = tempdir().unwrap();
        let root = d.path();
        fs::create_dir_all(root.join(".aida/sessions")).unwrap();
        fs::create_dir_all(root.join(".aida/headless-logs")).unwrap();
        fs::write(root.join(".aida/sessions/a.manifest.toml"), "session_id = \"a\"\nplanned_at = 2026-01-01T00:00:00Z\nplan_source = \"test\"\nclaude_session_id = \"fixture-session\"\n[[items]]\nspec_id = \"TASK-1427\"\nposition = 1\nstatus_at_plan = \"In Progress\"\n").unwrap();
        fs::write(
            root.join(".aida/sessions/a.toml"),
            "id = \"a\"\nrole = \"implementer\"\nscope = \"TASK-1427\"\n",
        )
        .unwrap();
        let phases = [
            ("2026-01-01T00:00:00Z", "implementer", 1),
            ("2026-01-01T00:00:10Z", "reviewer", 1),
            // A skipped attempt is authoritative; it must not be renumbered 2.
            ("2026-01-01T00:00:20Z", "implementer", 3),
        ].into_iter().map(|(ts, slug, attempt)| serde_json::json!({"ts":ts,"spec":"TASK-1427","run_uuid":"run-1","kind":{"event":"PhaseEntered","slug":slug,"attempt":attempt}}).to_string()).collect::<Vec<_>>().join("\n");
        fs::write(root.join(".aida/events.jsonl"), format!("{phases}\n")).unwrap();
        let row = |id: &str, timestamp: Option<&str>, usage: Value| {
            let mut value = serde_json::json!({"session_id":"fixture-session","uuid":id,"requestId":id,"message":{"id":id,"model":"fixture","usage":usage}});
            if let Some(ts) = timestamp {
                value["timestamp"] = Value::from(ts);
            }
            value.to_string()
        };
        let log = [
            row(
                "impl",
                Some("2026-01-01T00:00:05Z"),
                serde_json::json!({"input_tokens":0,"output_tokens":1}),
            ),
            row(
                "review",
                Some("2026-01-01T00:00:15Z"),
                serde_json::json!({"input_tokens":2,"output_tokens":0}),
            ),
            row(
                "rework",
                Some("2026-01-01T00:00:25Z"),
                serde_json::json!({"input_tokens":3,"output_tokens":4}),
            ),
            row("unknown", None, serde_json::json!({"output_tokens":1})),
        ]
        .join("\n");
        fs::write(
            root.join(".aida/headless-logs/fixture-session.jsonl"),
            format!("{log}\n"),
        )
        .unwrap();
        rebuild(root, false).unwrap();
        let conn = Connection::open(ledger_path(root)).unwrap();
        let payload = query_payload(&conn, "TASK-1427", "phase,vendor,round", false).unwrap();
        assert_eq!(
            payload.spec_coverage,
            SpecCoverage {
                candidate_records: 4,
                measured_records: 4,
                non_measured_records: 0,
                records_with_unknown_dimensions: 1
            }
        );
        assert_eq!(payload.groups.len(), 4);
        assert!(payload
            .groups
            .iter()
            .any(|g| g.phase.as_deref() == Some("implementer") && g.round == Some(3)));
        assert_eq!(payload.attribution_reasons["run"]["no_phase_evidence"], 1);
        assert_eq!(payload.attribution_reasons["phase"]["no_phase_evidence"], 1);
        assert_eq!(payload.attribution_reasons["round"]["no_round_evidence"], 1);
        assert!(payload.attribution_reasons["spec"].is_empty());
        let json = render_query(&payload, true, false).unwrap();
        let json_value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(json_value["spec_coverage"]["candidate_records"], 4);
        assert!(json_value["groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["input_uncached"]["total"].is_null()
                && g["input_uncached"]["missing_records"] == 1));
        assert!(json_value["groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |g| g["input_uncached"]["total"] == 0 && g["input_uncached"]["known_records"] == 1
            ));
        for rendered in [
            render_query(&payload, false, false).unwrap(),
            render_query(&payload, false, true).unwrap(),
        ] {
            assert!(rendered.contains("candidates=4"));
            assert!(rendered.contains("run: no_phase_evidence=1"));
            assert!(rendered.contains("spec: none"));
            assert!(rendered.contains("unknown (known 0, missing 1)"));
            assert!(rendered.contains("0 (known 1, missing 0)"));
            assert!(rendered.contains("Ledger-global collector diagnostics (not spec coverage)"));
        }
    }

    #[test]
    fn phase_attempts_preserve_missing_malformed_and_ambiguous_evidence() {
        let d = tempdir().unwrap();
        fs::create_dir_all(d.path().join(".aida")).unwrap();
        let rows = [
            serde_json::json!({"ts":"2026-01-01T00:00:00Z","spec":"TASK-1","run_uuid":"run","kind":{"event":"PhaseEntered","slug":"implementer","attempt":4}}),
            serde_json::json!({"ts":"2026-01-01T00:00:10Z","spec":"TASK-1","run_uuid":"run","kind":{"event":"PhaseEntered","slug":"reviewer"}}),
            serde_json::json!({"ts":"2026-01-01T00:00:20Z","spec":"TASK-1","run_uuid":"run","kind":{"event":"PhaseEntered","slug":"implementer","attempt":"bad"}}),
            serde_json::json!({"ts":"2026-01-01T00:00:30Z","spec":"TASK-1","run_uuid":"run","kind":{"event":"PhaseEntered","slug":"implementer","attempt":5}}),
            serde_json::json!({"ts":"2026-01-01T00:00:30Z","spec":"TASK-1","run_uuid":"run","kind":{"event":"PhaseEntered","slug":"reviewer","attempt":2}}),
        ].into_iter().map(|v| v.to_string()).collect::<Vec<_>>().join("\n");
        fs::write(d.path().join(".aida/events.jsonl"), format!("{rows}\n")).unwrap();
        let phases = load_phases(d.path());
        assert_eq!(phases[0].round, Some(4));
        assert_eq!(phases[1].round_reason.as_deref(), Some("missing_attempt"));
        assert_eq!(phases[2].round_reason.as_deref(), Some("malformed_attempt"));

        let mut records = vec![Record {
            record_id: "r".into(),
            vendor: "codex".into(),
            vendor_session_id: "v".into(),
            vendor_event_id: "e".into(),
            occurred_at: Some("2026-01-01T00:00:35Z".into()),
            ordinal: None,
            raw_model: None,
            source_basename: "v.jsonl".into(),
            source_hash: "h".into(),
            collection_status: "measured".into(),
            status_reason: None,
            tokens: Tokens::default(),
            vendor_usage: Value::Null,
            attribution: Attribution::default(),
        }];
        let manifests = vec![ManifestEvidence {
            aida_session_id: "a".into(),
            vendor_session_id: "v".into(),
            items: vec![ManifestItem {
                spec_id: "TASK-1".into(),
                started_at: None,
                completed_at: None,
            }],
            role: Some("implementer".into()),
        }];
        attribute(&mut records, &manifests, &phases, &mut Coverage::default());
        assert_eq!(
            records[0].attribution.phase_reason.as_deref(),
            Some("ambiguous_phase")
        );
        assert_eq!(
            records[0].attribution.round_reason.as_deref(),
            Some("ambiguous_round")
        );
        assert!(records[0].attribution.round.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_destination_and_oversize_source_fail_closed() {
        use std::os::unix::fs::symlink;
        let d = tempdir().unwrap();
        fs::create_dir_all(d.path().join(".aida")).unwrap();
        let outside = tempdir().unwrap();
        symlink(outside.path(), d.path().join(".aida/derived")).unwrap();
        assert!(prepare_ledger_destination(d.path())
            .unwrap_err()
            .to_string()
            .contains("symlinked"));
        let source = d.path().join("large.jsonl");
        let file = fs::File::create(&source).unwrap();
        file.set_len(MAX_SOURCE_BYTES + 1).unwrap();
        assert!(hash_regular_source(&source)
            .unwrap_err()
            .to_string()
            .contains("exceeds"));
    }
}
