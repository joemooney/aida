//! Explicit host constraint verification. Store data never becomes a command.
//! trace:STORY-1673 | ai:codex

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::{Requirement, RequirementType};

// An explicit null is malformed data, distinct from an absent verify block.
pub(crate) fn deserialize_verify<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    let yaml = serde_yaml::Value::deserialize(deserializer)?;
    // YAML tags, complex mapping keys and non-finite numbers are invalid
    // schema data, not a reason to drop the entire constraint on load. Keep
    // their textual value in a deliberately invalid block for diagnostics.
    if !json_compatible(&yaml) {
        return Ok(Some(serde_json::json!({
            "invalid_yaml": serde_yaml::to_string(&yaml).map_err(serde::de::Error::custom)?
        })));
    }
    serde_json::to_value(yaml)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

fn json_compatible(value: &serde_yaml::Value) -> bool {
    use serde_yaml::Value as Y;
    match value {
        Y::Tagged(_) => false,
        Y::Mapping(map) => map
            .iter()
            .all(|(key, value)| key.is_string() && json_compatible(value)),
        Y::Sequence(values) => values.iter().all(json_compatible),
        Y::Number(number) => number.as_f64().is_some_and(f64::is_finite),
        _ => true,
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Probe {
    DiskRotational,
    CpuCount,
    MemTotalBytes,
    PsiAvailable,
    PathExists,
    CommandOnPath,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Op {
    Eq,
    Ge,
    Le,
    Exists,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    op: Op,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Verify {
    version: u64,
    probe: Probe,
    args: BTreeMap<String, Value>,
    expect: Expect,
}

fn integer(value: &Value) -> Option<i128> {
    value
        .as_i64()
        .map(i128::from)
        .or_else(|| value.as_u64().map(i128::from))
}

impl Verify {
    fn parse(raw: &Value) -> Result<Self, String> {
        let verify: Self = serde_json::from_value(raw.clone())
            .map_err(|e| format!("invalid verify block: {e}"))?;
        if verify.version != 1 {
            return Err("unknown verify version; expected 1".into());
        }
        let key = match verify.probe {
            Probe::DiskRotational | Probe::PathExists => Some("path"),
            Probe::CommandOnPath => Some("name"),
            _ => None,
        };
        match key {
            Some(key) if verify.args.len() == 1 && verify.args.contains_key(key) => {
                let text = verify.args[key]
                    .as_str()
                    .ok_or("probe argument must be a string")?;
                if key == "name" {
                    if text.is_empty()
                        || !text
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                    {
                        return Err("command name must match [A-Za-z0-9._-]+".into());
                    }
                } else if !valid_path(text) {
                    return Err("path must be absolute, without parent traversal, controls or shell metacharacters".into());
                }
            }
            None if verify.args.is_empty() => {}
            _ => return Err("wrong or unknown probe arguments".into()),
        }
        match verify.probe {
            Probe::CpuCount | Probe::MemTotalBytes => {
                if integer(&verify.expect.value).is_none() || matches!(verify.expect.op, Op::Exists)
                {
                    return Err("numeric probe expects an integer and eq, ge or le".into());
                }
            }
            _ => {
                if !verify.expect.value.is_boolean()
                    || !matches!(verify.expect.op, Op::Eq | Op::Exists)
                {
                    return Err("boolean probe expects a boolean and eq or exists".into());
                }
            }
        }
        Ok(verify)
    }

    fn matches(&self, actual: &Value) -> bool {
        match self.expect.op {
            Op::Eq | Op::Exists => actual == &self.expect.value,
            Op::Ge => integer(actual)
                .zip(integer(&self.expect.value))
                .is_some_and(|(a, b)| a >= b),
            Op::Le => integer(actual)
                .zip(integer(&self.expect.value))
                .is_some_and(|(a, b)| a <= b),
        }
    }
}

fn valid_path(text: &str) -> bool {
    !text.is_empty()
        && Path::new(text).is_absolute()
        && !Path::new(text)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        && !text
            .chars()
            .any(|c| c.is_control() || "`$;&|<>\\".contains(c))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Match,
    Drift,
    Unknown,
    Invalid,
}

#[derive(Debug, Serialize)]
pub struct ConstraintResult {
    pub spec: String,
    pub title: String,
    pub probe: Option<String>,
    pub status: Status,
    pub expected: Option<Value>,
    pub observed: Option<Value>,
    pub detail: String,
}

/// Select constraint rows carrying a block. No status or store mutation.
/// Host access occurs only after the complete block has passed validation.
pub fn verify_constraints(requirements: &[Requirement]) -> Vec<ConstraintResult> {
    report_with(requirements, &LinuxHost::current())
}

fn report_with(requirements: &[Requirement], host: &LinuxHost) -> Vec<ConstraintResult> {
    let mut rows = Vec::new();
    for req in requirements
        .iter()
        .filter(|r| r.req_type == RequirementType::Constraint)
    {
        let Some(raw) = &req.verify else { continue };
        let mut row = ConstraintResult {
            spec: req.display_id(),
            title: req.title.clone(),
            probe: raw.get("probe").and_then(Value::as_str).map(str::to_owned),
            status: Status::Invalid,
            expected: raw.get("expect").and_then(|v| v.get("value")).cloned(),
            observed: None,
            detail: String::new(),
        };
        match Verify::parse(raw) {
            Err(error) => row.detail = error,
            Ok(verify) => match host.observe(&verify) {
                Err(error) => {
                    row.status = Status::Unknown;
                    row.detail = error;
                }
                Ok(value) => {
                    row.status = if verify.matches(&value) {
                        Status::Match
                    } else {
                        Status::Drift
                    };
                    row.detail = if row.status == Status::Match {
                        "expected value matched"
                    } else {
                        "host value differs"
                    }
                    .into();
                    row.observed = Some(value);
                }
            },
        }
        rows.push(row);
    }
    rows.sort_by(|a, b| a.spec.cmp(&b.spec));
    rows
}

// Private roots support deterministic procfs/sysfs fixtures without changing
// process environment or providing a new probe/harness extension surface.
struct LinuxHost {
    proc_root: PathBuf,
    sys_root: PathBuf,
    path: Option<OsString>,
}

impl LinuxHost {
    fn current() -> Self {
        Self {
            proc_root: "/proc".into(),
            sys_root: "/sys".into(),
            path: std::env::var_os("PATH"),
        }
    }

    fn observe(&self, verify: &Verify) -> Result<Value, String> {
        if !cfg!(target_os = "linux") {
            return Err("host probes require Linux".into());
        }
        match verify.probe {
            Probe::CpuCount => {
                let stat = self.read_proc("stat")?;
                let count = stat
                    .lines()
                    .filter_map(|l| l.split_whitespace().next())
                    .filter(|name| {
                        name.strip_prefix("cpu")
                            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                    })
                    .count();
                if count == 0 {
                    return Err("no logical CPU entries in procfs stat".into());
                }
                Ok(Value::from(count as u64))
            }
            Probe::MemTotalBytes => {
                let text = self.read_proc("meminfo")?;
                let mut values = text
                    .lines()
                    .filter_map(|line| line.strip_prefix("MemTotal:"));
                let value = values.next().ok_or("MemTotal unavailable")?;
                if values.next().is_some() {
                    return Err("duplicate MemTotal".into());
                }
                let parts: Vec<_> = value.split_whitespace().collect();
                if parts.len() != 2 || parts[1] != "kB" {
                    return Err("invalid MemTotal units".into());
                }
                let bytes = parts[0]
                    .parse::<u64>()
                    .ok()
                    .and_then(|kb| kb.checked_mul(1024))
                    .ok_or("invalid MemTotal size")?;
                Ok(Value::from(bytes))
            }
            Probe::PsiAvailable => {
                for name in ["cpu", "memory", "io"] {
                    let path = self.proc_root.join("pressure").join(name);
                    match std::fs::metadata(&path) {
                        Ok(m) if m.is_file() => {}
                        Ok(_) => return Err(format!("{} is not a file", path.display())),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            return Ok(Value::Bool(false))
                        }
                        Err(e) => return Err(format!("cannot inspect {}: {e}", path.display())),
                    }
                }
                Ok(Value::Bool(true))
            }
            Probe::PathExists => Path::new(verify.args["path"].as_str().unwrap())
                .try_exists()
                .map(Value::Bool)
                .map_err(|e| format!("cannot inspect path: {e}")),
            Probe::CommandOnPath => self
                .command_on_path(verify.args["name"].as_str().unwrap())
                .map(Value::Bool),
            Probe::DiskRotational => self
                .disk_rotational(Path::new(verify.args["path"].as_str().unwrap()))
                .map(Value::Bool),
        }
    }

    fn read_proc(&self, name: &str) -> Result<String, String> {
        std::fs::read_to_string(self.proc_root.join(name))
            .map_err(|e| format!("cannot read procfs {name}: {e}"))
    }

    fn command_on_path(&self, name: &str) -> Result<bool, String> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let path = self.path.as_ref().ok_or("PATH unavailable")?;
            let mut errors = Vec::new();
            for directory in std::env::split_paths(path) {
                let candidate = directory.join(name);
                match std::fs::metadata(&candidate) {
                    Ok(m) if m.is_file() => {
                        let name = std::ffi::CString::new(candidate.as_os_str().as_bytes())
                            .map_err(|_| "PATH contains a NUL byte")?;
                        // Check effective caller access, never execute the file.
                        // Any execute bit alone misclassifies owner/group denial.
                        // trace:STORY-1673 | ai:codex
                        if unsafe {
                            libc::faccessat(
                                libc::AT_FDCWD,
                                name.as_ptr(),
                                libc::X_OK,
                                libc::AT_EACCESS,
                            )
                        } == 0
                        {
                            return Ok(true);
                        }
                        let error = std::io::Error::last_os_error();
                        if !matches!(
                            error.kind(),
                            std::io::ErrorKind::PermissionDenied
                                | std::io::ErrorKind::NotFound
                                | std::io::ErrorKind::NotADirectory
                        ) {
                            errors.push(error.to_string());
                        }
                    }
                    Ok(_) => {}
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                        ) => {}
                    Err(e) => errors.push(e.to_string()),
                }
            }
            if errors.is_empty() {
                Ok(false)
            } else {
                Err(format!("PATH lookup unavailable: {}", errors.join("; ")))
            }
        }
        #[cfg(not(unix))]
        {
            Err("executable metadata unavailable".into())
        }
    }

    fn disk_rotational(&self, path: &Path) -> Result<bool, String> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::{FileTypeExt, MetadataExt};
            let metadata =
                std::fs::metadata(path).map_err(|e| format!("cannot inspect disk: {e}"))?;
            if !metadata.file_type().is_block_device() {
                return Err("disk path is not a block device".into());
            }
            let dev = metadata.rdev();
            let sys_path = self.sys_root.join("dev/block").join(format!(
                "{}:{}",
                libc::major(dev),
                libc::minor(dev)
            ));
            let disk = sys_path
                .canonicalize()
                .map_err(|e| format!("disk sysfs unavailable: {e}"))?;
            self.rotational_from_sysfs(&disk)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err("disk rotational state requires Linux".into())
        }
    }

    #[cfg(target_os = "linux")]
    fn rotational_from_sysfs(&self, disk: &Path) -> Result<bool, String> {
        let queue = if disk
            .join("partition")
            .try_exists()
            .map_err(|e| e.to_string())?
        {
            disk.parent()
                .ok_or("partition parent unavailable")?
                .join("queue/rotational")
        } else {
            disk.join("queue/rotational")
        };
        let text = std::fs::read_to_string(queue)
            .map_err(|e| format!("rotational state unavailable: {e}"))?;
        match text.trim() {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => Err("invalid rotational state".into()),
        }
    }
}

#[cfg(all(test, feature = "native"))]
#[path = "constraints_tests.rs"]
mod tests;
