//! STORY-1405: the "review in progress" marker.
//!
//! A per-PR file at the MAIN clone's `.aida/review-in-progress/PR-<N>` that a
//! reviewer writes when it STARTS on a PR and that is removed when its verdict
//! is recorded. Every merge surface (`aida pr ship`, the drain's merge phase)
//! consults it right before merging, and `aida awaiting` annotates a mergeable
//! PR that carries one — so a merge can no longer land seconds before the
//! verdict that would have refused it.
//!
//! Three states for a PR (STORY-1405 criterion 4): no marker and no verdict
//! ("no opinion"), a live marker ("under review"), a recorded refusing verdict
//! ("reviewed and refused" — the existing `merge_hold`, not this module).
//!
//! Keyed on PR number AND head sha: a marker for an older head does not block
//! a merge of the current head (that review is not about what would merge).
//! A marker with no recorded head is treated as covering every head — an
//! unknown head fails closed.
//!
//! Expiry (criterion 2): a marker written by a process that holds it for the
//! whole review (the drain's reviewer phase, `aida review <SPEC>`) records its
//! pid, and dies with it — a DEAD pid on this host expires it at once. Every
//! marker also carries a TTL backstop: 2h for pid-backed markers (a pid can be
//! reused), 30m (overridable) for an explicit `aida review claim`, which has no
//! process to watch. An expired marker is removed on read and the merger is
//! told so — it never blocks forever.
//!
//! Cost (criterion 7): with no marker present the gate is one `stat` of a
//! local file. Only a LIVE marker costs a forge round-trip, to compare the
//! marker's head against the PR's current head.
// trace:STORY-1405 | ai:claude

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// TTL backstop for a marker whose holder process is watched (pid-backed).
pub(crate) const PID_BACKED_TTL_SECS: u64 = 2 * 60 * 60;
/// Default TTL for an explicit `aida review claim` (no process to watch).
pub(crate) const CLAIM_TTL_SECS: u64 = 30 * 60;

fn markers_dir(root: &Path) -> PathBuf {
    root.join(".aida").join("review-in-progress")
}

pub(crate) fn marker_path(root: &Path, pr: u64) -> PathBuf {
    markers_dir(root).join(format!("PR-{pr}"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// One review-in-progress marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Marker {
    pub pr: u64,
    /// Head the reviewer is looking at; empty = unknown (covers every head).
    pub head_sha: String,
    pub spec: Option<String>,
    /// Who is reviewing (free text: seat / surface).
    pub by: String,
    pub host: String,
    /// Holder pid; `0` = no process to watch (TTL only).
    pub pid: u32,
    pub started_at: u64,
    pub ttl_secs: u64,
}

impl Marker {
    /// A marker for a review this process runs for its whole duration.
    pub(crate) fn for_this_process(
        pr: u64,
        head_sha: Option<&str>,
        spec: Option<&str>,
        by: &str,
    ) -> Self {
        Marker {
            pr,
            head_sha: head_sha.unwrap_or("").trim().to_string(),
            spec: spec.map(str::to_string),
            by: by.to_string(),
            host: crate::coordination::hostname(),
            pid: std::process::id(),
            started_at: now_secs(),
            ttl_secs: PID_BACKED_TTL_SECS,
        }
    }

    /// Short human description, used by refusals and `aida awaiting`.
    pub(crate) fn describe(&self) -> String {
        let age = now_secs().saturating_sub(self.started_at);
        let head = if self.head_sha.is_empty() {
            "an unrecorded head".to_string()
        } else {
            format!("head {}", &self.head_sha[..self.head_sha.len().min(9)])
        };
        let holder = if self.pid == 0 {
            "claimed".to_string()
        } else {
            format!("pid {} on {}", self.pid, self.host)
        };
        let left = self.ttl_secs.saturating_sub(age);
        format!(
            "{} started reviewing {head} {}m ago ({holder}; expires in {}m)",
            self.by,
            age / 60,
            left.div_ceil(60)
        )
    }
}

fn serialize(m: &Marker) -> String {
    format!(
        "pr={}\nhead_sha={}\nspec={}\nby={}\nhost={}\npid={}\nstarted_at={}\nttl_secs={}\n",
        m.pr,
        m.head_sha,
        m.spec.as_deref().unwrap_or(""),
        m.by.replace('\n', " "),
        m.host,
        m.pid,
        m.started_at,
        m.ttl_secs,
    )
}

/// Lenient parse — a garbled field falls back to a value that still expires
/// via the TTL (started_at 0 ⇒ long expired) rather than wedging.
fn parse(pr: u64, body: &str) -> Marker {
    let mut m = Marker {
        pr,
        head_sha: String::new(),
        spec: None,
        by: "a reviewer".to_string(),
        host: String::new(),
        pid: 0,
        started_at: 0,
        ttl_secs: CLAIM_TTL_SECS,
    };
    for line in body.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            match k.trim() {
                "head_sha" => m.head_sha = v.to_string(),
                "spec" if !v.is_empty() => m.spec = Some(v.to_string()),
                "by" if !v.is_empty() => m.by = v.to_string(),
                "host" => m.host = v.to_string(),
                "pid" => m.pid = v.parse().unwrap_or(0),
                "started_at" => m.started_at = v.parse().unwrap_or(0),
                "ttl_secs" => m.ttl_secs = v.parse().unwrap_or(CLAIM_TTL_SECS),
                _ => {}
            }
        }
    }
    m
}

/// Write (or replace) the marker for `m.pr`.
pub(crate) fn write(root: &Path, m: &Marker) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(markers_dir(root))?;
    let path = marker_path(root, m.pr);
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, serialize(m))?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Read the raw marker for `pr`, live or not.
pub(crate) fn read(root: &Path, pr: u64) -> Option<Marker> {
    std::fs::read_to_string(marker_path(root, pr))
        .ok()
        .map(|b| parse(pr, &b))
}

/// Remove the marker for `pr`. Returns whether one was there.
pub(crate) fn clear(root: &Path, pr: u64) -> bool {
    std::fs::remove_file(marker_path(root, pr)).is_ok()
}

/// Every marker on disk (live or not), sorted by PR.
pub(crate) fn list(root: &Path) -> Vec<Marker> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(markers_dir(root)) {
        for e in entries.flatten() {
            let name = e.file_name();
            let Some(n) = name
                .to_str()
                .and_then(|s| s.strip_prefix("PR-"))
                .and_then(|s| s.parse::<u64>().ok())
            else {
                continue;
            };
            if let Some(m) = read(root, n) {
                out.push(m);
            }
        }
    }
    out.sort_by_key(|m| m.pr);
    out
}

/// `Some(reason)` when the marker no longer stands for a live review.
pub(crate) fn expired_reason(m: &Marker, our_host: &str, now: u64) -> Option<String> {
    if m.pid != 0
        && !m.host.is_empty()
        && m.host == our_host
        && !aida_core::liveness::pid_is_alive(m.pid)
    {
        return Some(format!("its reviewer (pid {}) is no longer running", m.pid));
    }
    let age = now.saturating_sub(m.started_at);
    if age > m.ttl_secs {
        return Some(format!(
            "it is {}m old, past its {}m expiry",
            age / 60,
            m.ttl_secs / 60
        ));
    }
    None
}

/// Is `m` still a live review (not expired by pid or TTL)?
pub(crate) fn is_live(m: &Marker) -> bool {
    expired_reason(m, &crate::coordination::hostname(), now_secs()).is_none()
}

/// Does a marker recorded at `marker_head` cover the PR's current head?
/// An empty side is unknown and fails closed (covers). Abbreviations match
/// by prefix.
pub(crate) fn covers_head(marker_head: &str, current_head: Option<&str>) -> bool {
    let a = marker_head.trim().to_ascii_lowercase();
    let Some(b) = current_head.map(|s| s.trim().to_ascii_lowercase()) else {
        return true;
    };
    if a.is_empty() || b.is_empty() {
        return true;
    }
    a.starts_with(&b) || b.starts_with(&a)
}

/// What a merge surface should do about the review marker for one PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MergeGate {
    /// No marker — merge.
    Clear,
    /// A live review is running at this head — refuse.
    UnderReview(Marker),
    /// A marker existed but had expired; it was removed. Merge, and say so.
    ExpiredCleared(Marker, String),
    /// A live marker for a DIFFERENT head — that review is not about what
    /// would merge. Merge, and say so.
    OtherHead(Marker),
}

/// Consult the marker for `pr` at merge time. `current_head` is called only
/// when a live marker exists (it may cost a forge round-trip); returning
/// `None` (head unknown) fails closed.
pub(crate) fn merge_gate(
    root: &Path,
    pr: u64,
    current_head: impl FnOnce() -> Option<String>,
) -> MergeGate {
    let Some(m) = read(root, pr) else {
        return MergeGate::Clear;
    };
    if let Some(reason) = expired_reason(&m, &crate::coordination::hostname(), now_secs()) {
        clear(root, pr);
        return MergeGate::ExpiredCleared(m, reason);
    }
    let head = current_head();
    if covers_head(&m.head_sha, head.as_deref()) {
        MergeGate::UnderReview(m)
    } else {
        MergeGate::OtherHead(m)
    }
}

/// Read-only lookup for `aida awaiting`: the description of a LIVE marker on
/// `pr` covering `current_head`, else `None`. Never removes anything.
pub(crate) fn live_description(root: &Path, pr: u64, current_head: Option<&str>) -> Option<String> {
    let m = read(root, pr)?;
    (is_live(&m) && covers_head(&m.head_sha, current_head)).then(|| m.describe())
}

/// The refusal a merge surface prints for [`MergeGate::UnderReview`].
pub(crate) fn refusal_message(m: &Marker) -> String {
    format!(
        "PR-{pr} is under review — {desc}. Merging now could land seconds before a verdict \
         that refuses it. Wait for the verdict (`aida review record … --pr {pr}` clears this), \
         or, if that review was abandoned, release it with `aida review claim --pr {pr} --release` \
         and retry.",
        pr = m.pr,
        desc = m.describe(),
    )
}

/// The note a merge surface prints for the non-blocking outcomes.
pub(crate) fn proceed_note(gate: &MergeGate) -> Option<String> {
    match gate {
        MergeGate::ExpiredCleared(m, reason) => Some(format!(
            "removed an expired review-in-progress marker on PR-{} ({reason}) — proceeding",
            m.pr
        )),
        MergeGate::OtherHead(m) => Some(format!(
            "PR-{} has a review in progress for an older head ({}) — it does not cover the \
             head being merged; proceeding",
            m.pr,
            &m.head_sha[..m.head_sha.len().min(9)]
        )),
        _ => None,
    }
}

/// RAII guard: removes the marker on drop, but only if the file on disk is
/// still the one this guard wrote (a later claim by another seat survives).
#[derive(Debug)]
pub(crate) struct MarkerGuard {
    root: PathBuf,
    marker: Marker,
}

impl Drop for MarkerGuard {
    fn drop(&mut self) {
        if read(&self.root, self.marker.pr).as_ref() == Some(&self.marker) {
            clear(&self.root, self.marker.pr);
        }
    }
}

/// Write `m` and return a guard that removes it when the review ends.
/// Best-effort: a write failure is reported by the caller as a warning, never
/// fails the review itself.
pub(crate) fn hold(root: &Path, m: Marker) -> std::io::Result<MarkerGuard> {
    write(root, &m)?;
    Ok(MarkerGuard {
        root: root.to_path_buf(),
        marker: m,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(pr: u64, head: &str, started_at: u64, ttl: u64) -> Marker {
        Marker {
            pr,
            head_sha: head.to_string(),
            spec: Some("STORY-1".into()),
            by: "reviewer-1".into(),
            host: "some-host".into(),
            pid: 0,
            started_at,
            ttl_secs: ttl,
        }
    }

    #[test]
    fn no_marker_is_clear_and_never_asks_for_head() {
        let dir = tempfile::tempdir().unwrap();
        let gate = merge_gate(dir.path(), 7, || panic!("head lookup with no marker"));
        assert_eq!(gate, MergeGate::Clear);
    }

    #[test]
    fn live_marker_at_same_head_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let m = claim(7, "abcdef1234", now_secs(), CLAIM_TTL_SECS);
        write(dir.path(), &m).unwrap();
        let gate = merge_gate(dir.path(), 7, || Some("abcdef1234567890".into()));
        assert_eq!(gate, MergeGate::UnderReview(m.clone()));
        assert!(refusal_message(&m).contains("aida review claim --pr 7 --release"));
        // Still there: a refusal does not consume the marker.
        assert!(read(dir.path(), 7).is_some());
    }

    #[test]
    fn live_marker_with_unknown_current_head_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &claim(7, "abc", now_secs(), CLAIM_TTL_SECS)).unwrap();
        assert!(matches!(
            merge_gate(dir.path(), 7, || None),
            MergeGate::UnderReview(_)
        ));
    }

    #[test]
    fn marker_at_different_head_does_not_block() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &claim(7, "aaaa111", now_secs(), CLAIM_TTL_SECS)).unwrap();
        let gate = merge_gate(dir.path(), 7, || Some("bbbb222".into()));
        assert!(matches!(gate, MergeGate::OtherHead(_)));
        assert!(proceed_note(&gate).unwrap().contains("older head"));
    }

    #[test]
    fn ttl_expired_marker_does_not_block_and_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let old = now_secs() - CLAIM_TTL_SECS - 60;
        write(dir.path(), &claim(7, "abc", old, CLAIM_TTL_SECS)).unwrap();
        let gate = merge_gate(dir.path(), 7, || panic!("expired marker needs no head"));
        assert!(matches!(gate, MergeGate::ExpiredCleared(_, _)));
        assert!(proceed_note(&gate).unwrap().contains("expired"));
        assert!(read(dir.path(), 7).is_none());
    }

    #[test]
    fn dead_pid_on_this_host_expires_at_once() {
        let mut m = Marker::for_this_process(7, Some("abc"), None, "drain reviewer");
        // A pid far above any real pid_max.
        m.pid = 4_000_000_000;
        let reason = expired_reason(&m, &m.host.clone(), now_secs()).unwrap();
        assert!(reason.contains("no longer running"), "{reason}");
        // Our own live pid does not expire.
        let live = Marker::for_this_process(7, Some("abc"), None, "drain reviewer");
        assert!(expired_reason(&live, &live.host.clone(), now_secs()).is_none());
    }

    #[test]
    fn guard_releases_on_drop_but_spares_a_newer_claim() {
        let dir = tempfile::tempdir().unwrap();
        {
            let _g = hold(
                dir.path(),
                Marker::for_this_process(7, Some("abc"), None, "drain reviewer"),
            )
            .unwrap();
            assert!(read(dir.path(), 7).is_some());
        }
        assert!(read(dir.path(), 7).is_none());

        let g = hold(
            dir.path(),
            Marker::for_this_process(8, Some("abc"), None, "drain reviewer"),
        )
        .unwrap();
        let newer = claim(8, "def", now_secs(), CLAIM_TTL_SECS);
        write(dir.path(), &newer).unwrap();
        drop(g);
        assert_eq!(read(dir.path(), 8), Some(newer));
    }

    #[test]
    fn roundtrip_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let a = claim(9, "abc", 100, 60);
        let b = claim(3, "", 100, 60);
        write(dir.path(), &a).unwrap();
        write(dir.path(), &b).unwrap();
        assert_eq!(list(dir.path()), vec![b, a]);
    }
}
