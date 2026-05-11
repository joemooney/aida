//! Deterministic-serialization helpers for YAML output.
//!
//! HashMap and HashSet iterate in arbitrary order, which causes spurious diffs
//! when round-tripping YAML stored in git. These helpers serialize them in
//! sorted order so the on-disk representation is stable.
//!
//! trace:BUG-1-040 | ai:claude

use serde::ser::{SerializeMap, SerializeSeq, Serializer};
use std::collections::{HashMap, HashSet};

/// Serialize a `HashSet<String>` as a sorted sequence so YAML output is stable.
pub fn serialize_sorted_string_set<S>(
    set: &HashSet<String>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut sorted: Vec<&String> = set.iter().collect();
    sorted.sort();
    let mut seq = serializer.serialize_seq(Some(sorted.len()))?;
    for item in sorted {
        seq.serialize_element(item)?;
    }
    seq.end()
}

/// Serialize a `HashMap<String, String>` with sorted keys so YAML output is stable.
pub fn serialize_sorted_string_map<S>(
    map: &HashMap<String, String>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut sorted: Vec<(&String, &String)> = map.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut m = serializer.serialize_map(Some(sorted.len()))?;
    for (k, v) in sorted {
        m.serialize_entry(k, v)?;
    }
    m.end()
}
