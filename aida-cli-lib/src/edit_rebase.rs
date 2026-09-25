//! The `aida edit` write: rebase the edit onto the copy read under the store
//! write lock, then write that one spec.
//!
//! `aida edit` validates and builds its change against a copy of the spec it
//! read before the write (refusals, prompts, the editor buffer and the
//! STORY-1429 lease gate all run there). Writing that early copy back would
//! revert anything another writer changed in between. Instead the write goes
//! through the per-spec compare-and-swap (`update_spec_atomically`): under the
//! store lock the spec is re-read, the edit's field changes (read copy →
//! planned copy) are applied to that fresh copy, and only then is it written.
//!
//! Per field, against the copy the edit started from:
//! - a field the edit did not change keeps the current value (a concurrent
//!   change survives);
//! - a field only the edit changed takes the edit's value;
//! - a list (tags, comments, relationships, refs, …) changed on both sides
//!   keeps both: the edit's removals and additions are applied to the
//!   current list;
//! - a nested map is merged key by key under the same rules, except the
//!   struct-valued fields in [`ATOMIC_FIELDS`], which are one value each;
//! - a scalar both sides changed to different values is a real conflict: the
//!   edit is refused and nothing is written.
//!
//! Some edits refuse on any concurrent change instead of merging (strict
//! review, TASK-1506):
//! - `--tags` replaces the whole set, so tags are one value: a concurrent
//!   change to tags refuses (`--add-tag` / `--remove-tag` still merge);
//! - an edit that changes `status` refuses on a concurrent status change;
//! - an edit that leaves NeedsAttention refuses unless the spec is unchanged
//!   since it was read (every field but `modified_at`).
//!
//! `modified_at` always takes the edit's stamp.
// trace:TASK-1506 | ai:claude

use aida_core::{Requirement, RequirementStatus};
use serde_yaml::{Mapping, Value};

/// Struct-valued fields that are one value each: when both the edit and a
/// concurrent writer changed one, it is a conflict, not a key-wise merge.
/// (`implementation_info` and `custom_fields` stay key-merged.)
// trace:TASK-1506 | ai:claude
const ATOMIC_FIELDS: &[&str] = &[
    "failure_reason",
    "attention_reason",
    "decision_request",
    "interface_changes",
    "ai_evaluation",
    "filed_at",
    "origin",
];

/// How strictly an edit merges with a concurrent change.
// trace:TASK-1506 | ai:claude
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EditMerge {
    /// The edit used `--tags` (replace the whole set), so tags are one value.
    pub(crate) tags_replaced: bool,
    /// The edit leaves NeedsAttention: any concurrent change refuses.
    pub(crate) leaving_needs_attention: bool,
}

fn as_map(r: &Requirement) -> anyhow::Result<Mapping> {
    match serde_yaml::to_value(r)? {
        Value::Mapping(m) => Ok(m),
        _ => anyhow::bail!("a requirement did not serialize to a mapping"),
    }
}

/// Apply the edit (`read` → `planned`) to `current`, the copy read under the
/// store lock. Returns the requirement to write, or the list of fields both
/// the edit and a concurrent writer changed (see [`EditMerge`] for the edits
/// that refuse on any concurrent change to a field).
// trace:TASK-1506 | ai:claude
pub(crate) fn rebase_edit(
    read: &Requirement,
    planned: &Requirement,
    current: &Requirement,
    opts: EditMerge,
) -> anyhow::Result<Result<Requirement, Vec<String>>> {
    let (r, p, c) = (as_map(read)?, as_map(planned)?, as_map(current)?);
    let null = Value::Null;
    // Leaving NeedsAttention acts on the parked spec as it was read; any
    // concurrent change (a new tag, a comment, …) refuses.
    // trace:TASK-1506 | ai:claude
    if opts.leaving_needs_attention {
        let mut changed: Vec<String> = Vec::new();
        for k in c.keys().chain(r.keys()) {
            let Some(name) = k.as_str() else { continue };
            if name == "modified_at" || changed.iter().any(|n| n == name) {
                continue;
            }
            if r.get(k).unwrap_or(&null) != c.get(k).unwrap_or(&null) {
                changed.push(name.to_string());
            }
        }
        if !changed.is_empty() {
            return Ok(Err(changed));
        }
    }
    // `--tags` replaced the set: tags are one value, so a concurrent change
    // to them refuses. trace:TASK-1506 | ai:claude
    if opts.tags_replaced && current.tags != read.tags {
        return Ok(Err(vec!["tags".to_string()]));
    }
    let mut conflicts = Vec::new();
    let merged = merge_maps(&r, &p, &c, "", &mut conflicts);
    if !conflicts.is_empty() {
        return Ok(Err(conflicts));
    }
    Ok(Ok(serde_yaml::from_value(Value::Mapping(merged))?))
}

fn merge_maps(
    read: &Mapping,
    planned: &Mapping,
    current: &Mapping,
    prefix: &str,
    conflicts: &mut Vec<String>,
) -> Mapping {
    let mut keys: Vec<&Value> = Vec::new();
    for k in current.keys().chain(planned.keys()).chain(read.keys()) {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    let null = Value::Null;
    let mut out = Mapping::new();
    for k in keys {
        let name = match k.as_str() {
            Some(s) if prefix.is_empty() => s.to_string(),
            Some(s) => format!("{prefix}.{s}"),
            None => format!("{prefix}.?"),
        };
        let r = read.get(k).unwrap_or(&null);
        let p = planned.get(k).unwrap_or(&null);
        let c = current.get(k).unwrap_or(&null);
        let v = if name == "modified_at" {
            p.clone()
        } else if prefix.is_empty() && ATOMIC_FIELDS.contains(&name.as_str()) {
            // One value: both sides changing it differently is a conflict.
            // trace:TASK-1506 | ai:claude
            merge_atomic(r, p, c, &name, conflicts)
        } else {
            merge_value(r, p, c, &name, conflicts)
        };
        // An absent field stays absent, so serde's defaults apply on the way
        // back (a `null` would not deserialize into a list field).
        if !v.is_null() {
            out.insert(k.clone(), v);
        }
    }
    out
}

/// Merge a field that is one value: the side that changed it wins, and both
/// sides changing it to different values is a conflict.
// trace:TASK-1506 | ai:claude
fn merge_atomic(
    read: &Value,
    planned: &Value,
    current: &Value,
    name: &str,
    conflicts: &mut Vec<String>,
) -> Value {
    if planned == read || current == planned {
        current.clone()
    } else if current == read {
        planned.clone()
    } else {
        conflicts.push(name.to_string());
        current.clone()
    }
}

fn merge_value(
    read: &Value,
    planned: &Value,
    current: &Value,
    name: &str,
    conflicts: &mut Vec<String>,
) -> Value {
    if planned == read || current == planned {
        return current.clone();
    }
    if current == read {
        return planned.clone();
    }
    let empty_seq = Vec::new();
    let as_seq = |v: &'_ Value| -> Option<Vec<Value>> {
        match v {
            Value::Sequence(s) => Some(s.clone()),
            Value::Null => Some(empty_seq.clone()),
            _ => None,
        }
    };
    let is_seq = |v: &Value| matches!(v, Value::Sequence(_));
    if is_seq(planned) || is_seq(current) {
        if let (Some(r), Some(p), Some(c)) = (as_seq(read), as_seq(planned), as_seq(current)) {
            let mut out: Vec<Value> = c
                .into_iter()
                .filter(|item| !(r.contains(item) && !p.contains(item)))
                .collect();
            for item in p {
                if !r.contains(&item) && !out.contains(&item) {
                    out.push(item);
                }
            }
            return Value::Sequence(out);
        }
    }
    let empty_map = Mapping::new();
    let as_map = |v: &'_ Value| -> Option<Mapping> {
        match v {
            Value::Mapping(m) => Some(m.clone()),
            Value::Null => Some(empty_map.clone()),
            _ => None,
        }
    };
    if let (Value::Mapping(p), Value::Mapping(c)) = (planned, current) {
        if let Some(r) = as_map(read) {
            return Value::Mapping(merge_maps(&r, p, c, name, conflicts));
        }
    }
    conflicts.push(name.to_string());
    current.clone()
}

/// The refusal for a same-field conflict.
// trace:TASK-1506 | ai:claude
pub(crate) fn conflict_message(label: &str, fields: &[String]) -> String {
    format!(
        "{label} changed while this edit ran ({} also changed by another writer); nothing was \
         written. Re-check it with `aida show {label}` and re-run the edit.",
        fields.join(", ")
    )
}

/// Write an `aida edit` through the per-spec compare-and-swap: under the
/// store write lock, re-read the spec, re-check the NeedsAttention exit
/// (`leave`: the label and the status it returns to), rebase the edit onto
/// that copy and write it. Nothing is written on a refusal. Returns the
/// written requirement. `tags_replaced` is true when the edit used `--tags`
/// (replace), which refuses on a concurrent change to tags.
// trace:TASK-1506 trace:STORY-1429 | ai:claude
pub(crate) fn write_edit_atomically(
    backend: &aida_core::CachedGitBackend,
    label: &str,
    read: &Requirement,
    planned: &Requirement,
    leave: Option<&RequirementStatus>,
    tags_replaced: bool,
    commit_subject: Option<&str>,
) -> anyhow::Result<Requirement> {
    let opts = EditMerge {
        tags_replaced,
        leaving_needs_attention: leave.is_some(),
    };
    let mut refusal: Option<anyhow::Error> = None;
    let written = backend.update_spec_atomically_with_subject(read, commit_subject, |cur| {
        if let Some(target) = leave {
            if let Some(moved) = crate::requeue::recheck_before_write(
                Some(cur),
                &RequirementStatus::NeedsAttention,
                target,
            ) {
                refusal = Some(anyhow::anyhow!(crate::requeue::unchanged_message(
                    label, &moved
                )
                .unwrap_or_default()));
                return;
            }
        }
        match rebase_edit(read, planned, cur, opts) {
            Ok(Ok(merged)) => *cur = merged,
            Ok(Err(fields)) => refusal = Some(anyhow::anyhow!(conflict_message(label, &fields))),
            Err(e) => refusal = Some(e),
        }
    })?;
    if let Some(e) = refusal {
        return Err(e);
    }
    written.ok_or_else(|| {
        anyhow::anyhow!(crate::requeue::unchanged_message(
            label,
            &crate::requeue::ReturnOutcome::Missing
        )
        .unwrap_or_default())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::{Comment, DatabaseBackend, RequirementPriority as Priority};

    fn spec() -> Requirement {
        let mut r = Requirement::new("title".into(), "body".into());
        r.spec_id = Some("TASK-1".to_string());
        r.status = RequirementStatus::Approved;
        r.tags.insert("keep".to_string());
        r
    }

    // trace:TASK-1506 | ai:claude
    #[test]
    fn different_field_edits_both_survive() {
        let read = spec();
        let mut planned = read.clone();
        planned.title = "new title".into();
        planned.tags.insert("mine".into());
        planned.tags.remove("keep");
        let mut current = read.clone();
        current.priority = Priority::High;
        current.tags.insert("theirs".into());
        current.add_comment(Comment::new("other".into(), "concurrent note".into()));

        let merged = rebase_edit(&read, &planned, &current, EditMerge::default())
            .unwrap()
            .unwrap();
        assert_eq!(merged.title, "new title");
        assert_eq!(merged.priority, Priority::High);
        assert!(merged.tags.contains("mine"));
        assert!(merged.tags.contains("theirs"));
        assert!(!merged.tags.contains("keep"));
        assert_eq!(merged.comments.len(), 1);
    }

    // trace:TASK-1506 | ai:claude
    #[test]
    fn same_field_conflict_is_refused() {
        let read = spec();
        let mut planned = read.clone();
        planned.status = RequirementStatus::Completed;
        planned.owner = "me".into();
        let mut current = read.clone();
        current.status = RequirementStatus::InProgress;
        let fields = rebase_edit(&read, &planned, &current, EditMerge::default())
            .unwrap()
            .unwrap_err();
        assert_eq!(fields, vec!["status".to_string()]);
        // Both sides making the same change is not a conflict.
        current.status = RequirementStatus::Completed;
        let merged = rebase_edit(&read, &planned, &current, EditMerge::default())
            .unwrap()
            .unwrap();
        assert_eq!(merged.owner, "me");
    }

    // trace:TASK-1506 | ai:claude
    #[test]
    fn both_sides_appending_comments_keep_both() {
        let read = spec();
        let mut planned = read.clone();
        planned.add_comment(Comment::new("me".into(), "edit note".into()));
        let mut current = read.clone();
        current.add_comment(Comment::new("other".into(), "their note".into()));
        let merged = rebase_edit(&read, &planned, &current, EditMerge::default())
            .unwrap()
            .unwrap();
        assert_eq!(merged.comments.len(), 2);
    }

    // A struct-valued field both sides changed is a conflict, not a key-wise
    // merge; implementation_info-style maps still merge. trace:TASK-1506 | ai:claude
    #[test]
    fn struct_valued_field_changed_on_both_sides_is_a_conflict() {
        let read = spec();
        let mut planned = read.clone();
        planned.filed_at = Some(aida_core::FilingProvenance {
            code_sha: Some("abc".into()),
            ..Default::default()
        });
        let mut current = read.clone();
        current.filed_at = Some(aida_core::FilingProvenance {
            branch: Some("main".into()),
            ..Default::default()
        });
        let fields = rebase_edit(&read, &planned, &current, EditMerge::default())
            .unwrap()
            .unwrap_err();
        assert_eq!(fields, vec!["filed_at".to_string()]);
        // Only one side changed it: that side wins.
        let merged = rebase_edit(&read, &planned, &read, EditMerge::default())
            .unwrap()
            .unwrap();
        assert_eq!(merged.filed_at, planned.filed_at);
        // custom_fields stay key-merged.
        let mut planned = read.clone();
        planned.custom_fields.insert("a".into(), "1".into());
        let mut current = read.clone();
        current.custom_fields.insert("b".into(), "2".into());
        let merged = rebase_edit(&read, &planned, &current, EditMerge::default())
            .unwrap()
            .unwrap();
        assert_eq!(merged.custom_fields.len(), 2);
    }

    // `--tags` replace treats tags as one value; `--add-tag` still merges.
    // trace:TASK-1506 | ai:claude
    #[test]
    fn tags_replace_refuses_a_concurrent_tag_change() {
        let read = spec();
        let mut planned = read.clone();
        planned.tags.insert("x".into());
        let mut current = read.clone();
        current.tags.insert("c".into());
        let replace = EditMerge {
            tags_replaced: true,
            ..Default::default()
        };
        let fields = rebase_edit(&read, &planned, &current, replace)
            .unwrap()
            .unwrap_err();
        assert_eq!(fields, vec!["tags".to_string()]);
        let merged = rebase_edit(&read, &planned, &current, EditMerge::default())
            .unwrap()
            .unwrap();
        assert!(merged.tags.contains("x") && merged.tags.contains("c"));
        // No concurrent tag change: the replace applies.
        let mut current = read.clone();
        current.priority = Priority::High;
        let merged = rebase_edit(&read, &planned, &current, replace)
            .unwrap()
            .unwrap();
        assert_eq!(merged.tags, planned.tags);
        assert_eq!(merged.priority, Priority::High);
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        store_root: std::path::PathBuf,
        backend: aida_core::CachedGitBackend,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::TempDir::new().unwrap();
        let store_root = tmp.path().join(".aida-store");
        std::fs::create_dir_all(&store_root).unwrap();
        aida_core::git_ops::init(&store_root).unwrap();
        aida_core::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();
        let cache_path = tmp.path().join(".aida").join("cache.db");
        let backend = aida_core::CachedGitBackend::open(&store_root, &cache_path).unwrap();
        Fixture {
            _tmp: tmp,
            store_root,
            backend,
        }
    }

    fn on_disk(f: &Fixture, spec_id: &str) -> Requirement {
        aida_core::GitBackend::new(&f.store_root)
            .unwrap()
            .get_requirement_by_spec_id(spec_id)
            .unwrap()
            .unwrap()
    }

    // A writer that lands between the edit's read and its write keeps its
    // change; the edit lands on top. trace:TASK-1506 | ai:claude
    #[test]
    fn concurrent_change_between_read_and_write_is_not_lost() {
        let f = fixture();
        let read = f.backend.add_requirement(spec()).unwrap();
        let mut planned = read.clone();
        planned.title = "edited title".into();
        planned.modified_at = chrono::Utc::now();

        {
            let other = aida_core::GitBackend::new(&f.store_root).unwrap();
            let mut theirs = other.get_requirement(&read.id).unwrap().unwrap();
            theirs.owner = "someone-else".into();
            theirs.tags.insert("theirs".into());
            theirs.add_comment(Comment::new("other".into(), "concurrent".into()));
            other.update_requirement(&theirs).unwrap();
        }

        let written =
            write_edit_atomically(&f.backend, "TASK-1", &read, &planned, None, false, None)
                .unwrap();
        let disk = on_disk(&f, "TASK-1");
        for r in [&written, &disk] {
            assert_eq!(r.title, "edited title");
            assert_eq!(r.owner, "someone-else");
            assert!(r.tags.contains("theirs"));
            assert_eq!(r.comments.len(), 1);
        }
    }

    // The same field changed by both is detected and nothing is written.
    // trace:TASK-1506 | ai:claude
    #[test]
    fn concurrent_status_change_refuses_a_status_edit() {
        let f = fixture();
        let read = f.backend.add_requirement(spec()).unwrap();
        let mut planned = read.clone();
        planned.status = RequirementStatus::Completed;
        planned.title = "edited".into();

        let other = aida_core::GitBackend::new(&f.store_root).unwrap();
        let mut theirs = other.get_requirement(&read.id).unwrap().unwrap();
        theirs.status = RequirementStatus::InProgress;
        other.update_requirement(&theirs).unwrap();

        let err = write_edit_atomically(&f.backend, "TASK-1", &read, &planned, None, false, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("status"), "{err}");
        assert!(err.contains("nothing was written"), "{err}");
        let disk = on_disk(&f, "TASK-1");
        assert_eq!(disk.status, RequirementStatus::InProgress);
        assert_eq!(disk.title, "title");
    }

    // The NeedsAttention exit is re-checked under the lock with the
    // STORY-1429 message. trace:TASK-1506 trace:STORY-1429 | ai:claude
    #[test]
    fn needs_attention_exit_is_rechecked_under_the_lock() {
        let f = fixture();
        let mut parked = spec();
        parked.status = RequirementStatus::NeedsAttention;
        let read = f.backend.add_requirement(parked).unwrap();
        let mut planned = read.clone();
        planned.status = RequirementStatus::Approved;

        let other = aida_core::GitBackend::new(&f.store_root).unwrap();
        let mut theirs = other.get_requirement(&read.id).unwrap().unwrap();
        theirs.status = RequirementStatus::InProgress;
        other.update_requirement(&theirs).unwrap();

        let err = write_edit_atomically(
            &f.backend,
            "TASK-1",
            &read,
            &planned,
            Some(&RequirementStatus::Approved),
            false,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("moved from"), "{err}");
        assert_eq!(on_disk(&f, "TASK-1").status, RequirementStatus::InProgress);

        // Still parked: the exit applies.
        theirs.status = RequirementStatus::NeedsAttention;
        other.update_requirement(&theirs).unwrap();
        let written = write_edit_atomically(
            &f.backend,
            "TASK-1",
            &read,
            &planned,
            Some(&RequirementStatus::Approved),
            false,
            Some("update TASK-1: test subject"),
        )
        .unwrap();
        assert_eq!(written.status, RequirementStatus::Approved);
        assert_eq!(on_disk(&f, "TASK-1").status, RequirementStatus::Approved);
        let log = std::process::Command::new("git")
            .arg("-C")
            .arg(&f.store_root)
            .args(["log", "-1", "--format=%s"])
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&log.stdout).contains("test subject"));
    }

    // Review probe (a): the read copy has tags {keep}; the user runs
    // `--tags keep,x` while another writer removes keep and adds c. The
    // replace refuses and nothing is written. trace:TASK-1506 | ai:claude
    #[test]
    fn probe_tags_replace_with_concurrent_tag_change_writes_nothing() {
        let f = fixture();
        let read = f.backend.add_requirement(spec()).unwrap();
        let mut planned = read.clone();
        planned.tags = ["keep", "x"].iter().map(|t| t.to_string()).collect();
        planned.modified_at = chrono::Utc::now();

        let other = aida_core::GitBackend::new(&f.store_root).unwrap();
        let mut theirs = other.get_requirement(&read.id).unwrap().unwrap();
        theirs.tags.remove("keep");
        theirs.tags.insert("c".into());
        other.update_requirement(&theirs).unwrap();

        let err = write_edit_atomically(&f.backend, "TASK-1", &read, &planned, None, true, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("tags"), "{err}");
        assert!(err.contains("nothing was written"), "{err}");
        assert!(err.contains("aida show TASK-1"), "{err}");
        let disk = on_disk(&f, "TASK-1");
        let want: std::collections::HashSet<String> = ["c".to_string()].into_iter().collect();
        assert_eq!(disk.tags, want);
    }

    // Review probe (b): the read copy is NeedsAttention; the edit sets
    // Approved with a return comment while another writer adds the
    // needs-human tag and a comment. The exit refuses and nothing is written.
    // trace:TASK-1506 | ai:claude
    #[test]
    fn probe_leaving_needs_attention_refuses_any_concurrent_change() {
        let f = fixture();
        let mut parked = spec();
        parked.status = RequirementStatus::NeedsAttention;
        let read = f.backend.add_requirement(parked).unwrap();
        let mut planned = read.clone();
        planned.status = RequirementStatus::Approved;
        planned.add_comment(Comment::new("me".into(), "returned to the queue".into()));
        planned.modified_at = chrono::Utc::now();

        let other = aida_core::GitBackend::new(&f.store_root).unwrap();
        let mut theirs = other.get_requirement(&read.id).unwrap().unwrap();
        theirs.tags.insert("needs-human".into());
        theirs.add_comment(Comment::new("other".into(), "needs a human".into()));
        other.update_requirement(&theirs).unwrap();

        let err = write_edit_atomically(
            &f.backend,
            "TASK-1",
            &read,
            &planned,
            Some(&RequirementStatus::Approved),
            false,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("nothing was written"), "{err}");
        assert!(err.contains("tags") && err.contains("comments"), "{err}");
        let disk = on_disk(&f, "TASK-1");
        assert_eq!(disk.status, RequirementStatus::NeedsAttention);
        assert_eq!(disk.comments.len(), 1);
        assert!(disk.tags.contains("needs-human"));
    }
}
