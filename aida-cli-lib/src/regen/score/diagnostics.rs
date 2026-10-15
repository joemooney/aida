//! Optional oracle diagnostics: missing/invalid evidence is unknown, never guessed.
// trace:STORY-1676 | ai:codex
use super::*;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;

const MAX_BYTES: u64 = 1024 * 1024;
pub(super) fn default_crash_share() -> u8 {
    50
}

pub(super) fn valid_family(family: &str) -> bool {
    !family.is_empty()
        && family.len() <= 128
        && family.trim() == family
        && !family.chars().any(char::is_control)
}

pub(super) struct Sidecar(std::fs::File);
impl Sidecar {
    pub(super) fn new(path: &Path) -> Result<Self> {
        Ok(Self(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?,
        ))
    }

    pub(super) fn read(self) -> Option<Vec<u8>> {
        let mut bytes = Vec::new();
        self.0.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
        (bytes.len() as u64 <= MAX_BYTES).then_some(bytes)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Crash,
    Assertion,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Detail {
    kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit: Option<i32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Line {
    v: u32,
    check: String,
    kind: Kind,
    exit: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    format_version: u32,
    crash_failure_share_percent: u8,
    classified: BTreeMap<String, Detail>,
}

pub(super) fn capture(bytes: Option<&[u8]>, checks: &[CheckOutcome], threshold: u8) -> Snapshot {
    let parse = || -> Option<BTreeMap<String, Detail>> {
        let bytes = bytes?;
        if bytes.len() as u64 > MAX_BYTES {
            return None;
        }
        let text = std::str::from_utf8(bytes).ok()?;
        let mut classified = BTreeMap::new();
        for line in text.lines() {
            let line: Line = serde_json::from_str(line).ok()?;
            if line.v != 1
                || !checks.iter().any(|c| c.name == line.check && !c.passed)
                || classified
                    .insert(
                        line.check,
                        Detail {
                            kind: line.kind,
                            exit: line.exit,
                        },
                    )
                    .is_some()
            {
                return None;
            }
        }
        Some(classified)
    };
    Snapshot {
        format_version: 1,
        crash_failure_share_percent: threshold,
        classified: parse().unwrap_or_default(),
    }
}

pub(super) struct Summary {
    pub(super) crashes: usize,
    pub(super) assertions: usize,
    pub(super) unknown: usize,
    pub(super) threshold: u8,
}
impl Summary {
    pub(super) fn warnings(&self) -> Vec<&'static str> {
        let mut flags = Vec::new();
        let classified = self.crashes + self.assertions;
        if classified > 0
            && self.crashes as u128 * 100 > classified as u128 * self.threshold as u128
        {
            flags.push("crash-dominated: review before reporting");
        }
        if self.unknown as u128 * 2 > (classified + self.unknown) as u128 {
            flags.push("unclassified: review before reporting");
        }
        flags
    }
}

pub(super) fn summary(record: &TrialRecord) -> Summary {
    let snapshot = record
        .failure_diagnostics
        .as_ref()
        .and_then(|v| serde_json::from_value::<Snapshot>(v.clone()).ok())
        .filter(|s| {
            s.format_version == 1
                && s.crash_failure_share_percent <= 100
                && s.classified
                    .keys()
                    .all(|name| record.checks.iter().any(|c| &c.name == name && !c.passed))
        });
    let mut counts = Summary {
        crashes: 0,
        assertions: 0,
        unknown: record.checks.iter().filter(|c| !c.passed).count(),
        threshold: snapshot
            .as_ref()
            .map_or(50, |s| s.crash_failure_share_percent),
    };
    if let Some(snapshot) = snapshot {
        for detail in snapshot.classified.values() {
            match detail.kind {
                Kind::Crash => counts.crashes += 1,
                Kind::Assertion => counts.assertions += 1,
            }
            counts.unknown -= 1;
        }
    }
    counts
}

// Attach quality warnings only after execution/control validation. Controls
// retain diagnostics without turning expected stub failures into dirty execution.
pub(super) fn flag_trial(record: &mut TrialRecord) {
    if record.status == "scored" && matches!(record.kind.as_str(), "trial" | "trial-adapted") {
        for warning in summary(record).warnings() {
            if !record.flags.iter().any(|f| f == warning) {
                record.flags.push(warning.into());
            }
        }
        record.clean = record.clean && record.flags.is_empty();
    }
    if let Some(adapter) = &mut record.adapter {
        adapter.r2.model_family = record.model_family.clone();
        flag_trial(&mut adapter.r2);
        record.clean = record.clean && adapter.r2.clean;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row() -> TrialRecord {
        serde_json::from_value(json!({
            "format_version":1,"trial_id":Uuid::new_v4(),"store_sha":"s","content_sha256":"c",
            "oracle_sha":"o","oracle_bundle_sha256":"b","model":"m","vendor":"v","harness":"h",
            "started_at":Utc::now(),"completed_at":Utc::now(),"status":"scored","clean":true,
            "checks":[{"name":"one","passed":false},{"name":"two","passed":false},{"name":"three","passed":false},
                      {"name":"Traceback exit 127","passed":false},{"name":"pass","passed":true}],
            "guard_hits":[],"unconfined":false,"flags":[]
        })).unwrap()
    }
    fn set(row: &mut TrialRecord, bytes: Option<&[u8]>, threshold: u8) {
        row.failure_diagnostics =
            Some(serde_json::to_value(capture(bytes, &row.checks, threshold)).unwrap());
    }

    #[test]
    fn diagnostics_are_explicit_and_invalid_sidecars_make_every_failure_unknown() {
        let mut row = row();
        for bytes in [None, Some(&b""[..]), Some(&b"garbage"[..]), Some(&b"\xff"[..]),
            Some(&b"{\"v\":2,\"check\":\"one\",\"kind\":\"crash\"}"[..]),
            Some(&b"{\"v\":1,\"check\":\"pass\",\"kind\":\"crash\"}"[..]),
            Some(&b"{\"v\":1,\"check\":\"missing\",\"kind\":\"crash\"}"[..]),
            Some(&b"{\"v\":1,\"check\":\"one\",\"kind\":\"unknown\"}"[..]),
            Some(&b"{\"v\":1,\"check\":\"one\",\"kind\":\"crash\",\"extra\":true}"[..]),
            Some(&b"{\"v\":1,\"check\":\"one\",\"kind\":\"crash\"}\ninvalid"[..]),
            Some(&b"{\"v\":1,\"check\":\"one\",\"kind\":\"crash\"}\n{\"v\":1,\"check\":\"one\",\"kind\":\"assertion\"}"[..]),
        ] {
            set(&mut row, bytes, 50);
            let summary = summary(&row);
            assert_eq!((summary.crashes, summary.assertions, summary.unknown), (0,0,4));
            assert_eq!(summary.warnings(), ["unclassified: review before reporting"]);
        }
        // Exit 127 and a traceback-looking name are not classification evidence.
        set(
            &mut row,
            Some(b"{\"v\":1,\"check\":\"Traceback exit 127\",\"kind\":\"assertion\",\"exit\":127}"),
            50,
        );
        let counts = summary(&row);
        assert_eq!(
            (counts.crashes, counts.assertions, counts.unknown),
            (0, 1, 3)
        );
        row.failure_diagnostics = Some(json!({"format_version":999}));
        assert_eq!(summary(&row).unknown, 4);
    }

    #[test]
    fn crash_share_uses_classified_denominator_and_strict_thresholds() {
        let mut row = row();
        let sidecar = b"{\"v\":1,\"check\":\"one\",\"kind\":\"crash\",\"exit\":126}\n{\"v\":1,\"check\":\"two\",\"kind\":\"assertion\"}\n";
        set(&mut row, Some(sidecar), 50);
        assert!(summary(&row).warnings().is_empty()); // 50% classified crashes, 50% unknown
        set(&mut row, Some(sidecar), 49);
        assert_eq!(
            summary(&row).warnings(),
            ["crash-dominated: review before reporting"]
        );
        set(
            &mut row,
            Some(b"{\"v\":1,\"check\":\"one\",\"kind\":\"crash\"}"),
            50,
        );
        assert_eq!(
            summary(&row).warnings(),
            [
                "crash-dominated: review before reporting",
                "unclassified: review before reporting"
            ]
        );
        flag_trial(&mut row);
        assert!(!row.clean);
        assert_eq!(row.flags.len(), 2);
        flag_trial(&mut row);
        assert_eq!(row.flags.len(), 2);
        row.checks.clear();
        row.failure_diagnostics = None;
        assert!(summary(&row).warnings().is_empty());
    }

    #[test]
    fn sidecar_reads_original_inode_and_bounds_bytes() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("results");
        let sidecar = Sidecar::new(&path).unwrap();
        std::fs::write(&path, "original").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", &path).unwrap();
        assert_eq!(sidecar.read().unwrap(), b"original");
        assert!(Sidecar::new(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        let mut sidecar = Sidecar::new(&path).unwrap();
        sidecar
            .0
            .write_all(&vec![b'x'; (MAX_BYTES + 1) as usize])
            .unwrap();
        // Simulate a separate oracle writer, without changing the reader offset.
        use std::io::{Seek, SeekFrom};
        sidecar.0.seek(SeekFrom::Start(0)).unwrap();
        assert!(sidecar.read().is_none());
    }

    #[test]
    fn nested_diagnostic_warnings_propagate_to_parent_cleanliness() {
        let mut r1 = row();
        for check in &mut r1.checks {
            check.passed = true;
        }
        r1.model_family = Some("explicit-family".into());
        let mut r2 = row();
        r2.kind = "trial-adapted".into();
        set(
            &mut r2,
            Some(b"{\"v\":1,\"check\":\"one\",\"kind\":\"crash\"}"),
            50,
        );
        r1.adapter = Some(
            serde_json::from_value(json!({
                "format_version":1, "adapter_input_sha256":"hash", "mappings":[], "r1":{},
                "r2":r2, "raw_delta":null, "discriminating_delta":null
            }))
            .unwrap(),
        );
        flag_trial(&mut r1);
        assert!(!r1.clean);
        assert!(
            r1.flags.is_empty(),
            "R2 warning must not be misattributed to R1 checks"
        );
        let r2 = &r1.adapter.as_ref().unwrap().r2;
        assert!(!r2.clean);
        assert!(r2.flags.iter().any(|f| f.starts_with("crash-dominated:")));
        assert_eq!(r2.model_family.as_deref(), Some("explicit-family"));
    }

    #[test]
    fn committed_reader_derives_legacy_quality_warnings_without_rewriting_evidence() {
        let store = tempfile::tempdir().unwrap();
        for args in [
            &["init", "-b", "aida-store"][..],
            &["config", "user.name", "Test"],
            &["config", "user.email", "test@example.com"],
        ] {
            super::super::super::git(store.path(), args).unwrap();
        }
        super::super::super::git(store.path(), &["commit", "--allow-empty", "-m", "fixture"])
            .unwrap();
        let mut legacy = row();
        legacy.aggregate = Some(Aggregate {
            passed: 1,
            total: 5,
            pass_rate: 0.2,
        });
        legacy.discriminating = Some(Aggregate {
            passed: 1,
            total: 5,
            pass_rate: 0.2,
        });
        super::super::persist(store.path(), &legacy).unwrap();
        let sha = super::super::super::head(store.path()).unwrap();
        let rows = controls::read_records(store.path()).unwrap();
        assert!(!rows[0].clean);
        assert_eq!(rows[0].flags, ["unclassified: review before reporting"]);
        let path = format!("regen/trials/{}.json", legacy.trial_id);
        let original: TrialRecord = serde_json::from_slice(
            &super::super::super::git(store.path(), &["show", &format!("HEAD:{path}")]).unwrap(),
        )
        .unwrap();
        assert!(
            original.clean && original.flags.is_empty() && original.failure_diagnostics.is_none()
        );
        assert_eq!(super::super::super::head(store.path()).unwrap(), sha);
    }

    #[test]
    fn model_family_is_explicit_validated_metadata() {
        for invalid in [
            "",
            " ",
            " family",
            "family ",
            "two\nfamilies",
            "x\u{1b}color",
        ] {
            assert!(!valid_family(invalid));
        }
        assert!(!valid_family(&"x".repeat(129)));
        assert!(valid_family("explicit family"));
        assert!(row().model_family.is_none());
    }
}
