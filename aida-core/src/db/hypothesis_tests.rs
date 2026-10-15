//! Adversarial canonical mutation coverage for pre-registration.
// trace:STORY-1675 | ai:codex
use super::{DatabaseBackend, GitBackend};
use crate::models::{
    Comment, Relationship, RelationshipType, Requirement, RequirementStatus, RequirementType,
    RequirementsStore,
};
use sha2::{Digest, Sha256};

fn rows() -> (Requirement, Requirement) {
    let mut h = Requirement::new("prediction".into(), "before result: λ".into());
    h.spec_id = Some("SPIKE-1".into());
    h.req_type = RequirementType::Spike;
    h.tags.insert("hypothesis".into());
    let mut r = Requirement::new("experiment".into(), "result".into());
    r.spec_id = Some("TASK-2".into());
    h.relationships.push(Relationship {
        rel_type: RelationshipType::References,
        target_id: r.id,
        created_at: None,
        created_by: None,
    });
    (h, r)
}
fn fixture() -> (tempfile::TempDir, GitBackend, Requirement, Requirement) {
    let tmp = tempfile::tempdir().unwrap();
    let b = GitBackend::new(&tmp.path().join("store"))
        .unwrap()
        .with_auto_commit(false)
        .with_oplog(false);
    let (h, r) = rows();
    let mut s = RequirementsStore::new();
    s.requirements = vec![h.clone(), r.clone()];
    b.save(&s).unwrap();
    (tmp, b, h, r)
}
fn disk(b: &GitBackend, r: &Requirement) -> Requirement {
    b.get_requirement(&r.id).unwrap().unwrap()
}

#[test]
fn hypothesis_all_mutation_routes_stamp_first_result_and_refuse_rewrites() {
    for route in 0..6 {
        let (_tmp, b, h, mut r) = fixture();
        r.status = RequirementStatus::Completed;
        match route {
            0 => b.update_requirement(&r).unwrap(),
            1 => {
                b.bulk_update(&[r.clone()], "complete").unwrap();
            }
            2 => {
                b.update_spec_atomically(&r, |r| r.status = RequirementStatus::Completed)
                    .unwrap();
            }
            3 => {
                b.bulk_update_atomically(&[r.clone()], "complete", |r| {
                    r.status = RequirementStatus::Completed;
                    true
                })
                .unwrap();
            }
            4 => {
                b.update_atomically(|s| {
                    s.requirements
                        .iter_mut()
                        .find(|q| q.id == r.id)
                        .unwrap()
                        .status = RequirementStatus::Completed
                })
                .unwrap();
            }
            _ => {
                let mut s = b.load().unwrap();
                s.requirements
                    .iter_mut()
                    .find(|q| q.id == r.id)
                    .unwrap()
                    .status = RequirementStatus::Completed;
                b.save(&s).unwrap();
            }
        }
        let locked = disk(&b, &h);
        let stamp = locked
            .hypothesis_lock
            .clone()
            .unwrap_or_else(|| panic!("route {route} did not stamp"));
        assert_eq!(
            stamp.description_sha256,
            format!("{:x}", Sha256::digest(h.description.as_bytes()))
        );
        assert!(stamp.locked_at >= h.created_at);
        let mut rewrite = locked.clone();
        rewrite.description = "post hoc prediction".into();
        let err = b.update_requirement(&rewrite).unwrap_err().to_string();
        assert!(err.contains("aida comment add SPIKE-1"), "{err}");
        assert_eq!(disk(&b, &h).description, h.description);
        // No unlock through any of these otherwise allowed edits, or a stale
        // whole-object copy with no lock stamp.
        let mut allowed = locked.clone();
        allowed.tags.clear();
        allowed.relationships.clear();
        allowed.title = "new title".into();
        allowed.hypothesis_lock = None;
        allowed
            .comments
            .push(Comment::new("test".into(), "amendment".into()));
        b.update_requirement(&allowed).unwrap();
        r.status = RequirementStatus::Approved;
        b.update_requirement(&r).unwrap();
        assert_eq!(disk(&b, &h).hypothesis_lock, Some(stamp));
        allowed.description = "still forbidden".into();
        assert!(b.update_requirement(&allowed).is_err());
    }
}

#[test]
fn hypothesis_both_edge_directions_and_types_lock_but_other_edges_and_legacy_do_not() {
    for reverse in [false, true] {
        for edge in [
            RelationshipType::References,
            RelationshipType::VerifiedBy,
            RelationshipType::Parent,
        ] {
            for tagged in [false, true] {
                let (_tmp, b, mut h, mut r) = fixture();
                h.req_type = RequirementType::Decision;
                h.relationships.clear();
                if !tagged {
                    h.tags.clear();
                }
                if reverse {
                    r.relationships.push(Relationship {
                        rel_type: edge.clone(),
                        target_id: h.id,
                        created_at: None,
                        created_by: None,
                    });
                } else {
                    h.relationships.push(Relationship {
                        rel_type: edge.clone(),
                        target_id: r.id,
                        created_at: None,
                        created_by: None,
                    });
                }
                b.bulk_update(&[h.clone(), r.clone()], "configure").unwrap();
                r.status = RequirementStatus::Completed;
                b.update_requirement(&r).unwrap();
                assert_eq!(
                    disk(&b, &h).hypothesis_lock.is_some(),
                    tagged && edge != RelationshipType::Parent
                );
            }
        }
    }
}

#[test]
fn hypothesis_pre_result_edits_allowed_and_refusal_preflights_all_objects() {
    for route in 0..4 {
        let (_tmp, b, mut h, r) = fixture();
        h.description = "revised before experiment".into();
        b.update_requirement(&h).unwrap();
        let before_h = disk(&b, &h);
        let before_r = disk(&b, &r);
        h.description = "changed while completing".into();
        let mut completed = r.clone();
        completed.status = RequirementStatus::Completed;
        let err = match route {
            0 => b
                .bulk_update(&[completed, h.clone()], "bad batch")
                .map(|_| ()),
            1 => b
                .update_atomically(|s| {
                    s.name = "must not write metadata".into();
                    for row in &mut s.requirements {
                        if row.id == r.id {
                            row.status = RequirementStatus::Completed;
                        }
                        if row.id == h.id {
                            row.description = h.description.clone();
                        }
                    }
                })
                .map(|_| ()),
            2 => {
                let mut s = b.load().unwrap();
                s.name = "must not write metadata".into();
                s.requirements = vec![completed, h.clone()];
                b.save(&s)
            }
            _ => b
                .bulk_update_atomically(&[r.clone(), h.clone()], "bad batch", |row| {
                    if row.id == r.id {
                        row.status = RequirementStatus::Completed;
                    } else {
                        row.description = h.description.clone();
                    }
                    true
                })
                .map(|_| ()),
        }
        .unwrap_err();
        assert!(err.to_string().contains("aida comment add"), "{err}");
        assert_eq!(
            serde_yaml::to_string(&disk(&b, &h)).unwrap(),
            serde_yaml::to_string(&before_h).unwrap()
        );
        assert_eq!(
            serde_yaml::to_string(&disk(&b, &r)).unwrap(),
            serde_yaml::to_string(&before_r).unwrap()
        );
        assert_ne!(b.load().unwrap().name, "must not write metadata");
    }
}

#[test]
fn hypothesis_existing_result_then_unlink_or_reopen_still_materializes_lock() {
    for route in 0..3 {
        let (_tmp, b, h, mut r) = fixture();
        // Simulate an old binary's unstamped completed result.
        r.status = RequirementStatus::Completed;
        crate::object_store::write_object(&b.path().join("objects"), &r).unwrap();
        match route {
            0 => {
                let mut h = h.clone();
                h.relationships.clear();
                h.tags.clear();
                b.update_requirement(&h).unwrap();
            }
            1 => {
                r.status = RequirementStatus::Approved;
                b.update_requirement(&r).unwrap();
            }
            _ => b.delete_requirement(&r.id).unwrap(),
        }
        assert!(disk(&b, &h).hypothesis_lock.is_some(), "route {route}");
        let mut h = disk(&b, &h);
        h.description = "too late".into();
        assert!(b.update_requirement(&h).is_err());
    }
}

#[test]
fn hypothesis_creation_and_import_lock_companion_rows() {
    for batch in [false, true] {
        let (_tmp, b, h, _) = fixture();
        let mut result = Requirement::new("new result".into(), "data".into());
        result.spec_id = Some("TASK-3".into());
        result.status = RequirementStatus::Completed;
        result.relationships.push(Relationship {
            rel_type: RelationshipType::VerifiedBy,
            target_id: h.id,
            created_at: None,
            created_by: None,
        });
        if batch {
            let mut writer = b.bulk_writer().unwrap();
            writer.add(result).unwrap();
            writer.finish("import").unwrap();
        } else {
            b.add_requirement(result).unwrap();
        }
        assert!(disk(&b, &h).hypothesis_lock.is_some());
    }
}

#[test]
fn hypothesis_duplicate_batch_cannot_hide_a_rewrite_or_transient_completion() {
    let (_tmp, b, h, mut r) = fixture();
    r.status = RequirementStatus::Completed;
    b.update_requirement(&r).unwrap();
    let locked = disk(&b, &h);
    let mut bad = locked.clone();
    bad.description = "post hoc".into();
    for batch in [
        vec![bad.clone(), locked.clone()],
        vec![locked.clone(), bad.clone()],
    ] {
        assert!(b
            .bulk_update(&batch, "duplicate")
            .unwrap_err()
            .to_string()
            .contains("duplicate"));
        assert_eq!(disk(&b, &h).description, h.description);
    }
    let (_tmp, b, h, r) = fixture();
    let mut completed = r.clone();
    completed.status = RequirementStatus::Completed;
    assert!(b
        .bulk_update(&[completed.clone(), r.clone()], "duplicate")
        .is_err());
    assert!(disk(&b, &h).hypothesis_lock.is_none());
    let err = b
        .update_atomically(|s| {
            let mut duplicate = completed;
            duplicate.spec_id = Some("TASK-3".into());
            s.requirements.push(duplicate);
        })
        .unwrap_err();
    assert!(err.to_string().contains("duplicate"));
    assert_eq!(disk(&b, &r).status, r.status);
}

#[test]
fn hypothesis_structured_merge_preserves_stamp_and_frozen_description_in_both_orders() {
    use crate::conflict::{merge_spec_three_way, resolve_conflict, Resolution};
    let (_tmp, b, h, mut r) = fixture();
    r.status = RequirementStatus::Completed;
    b.update_requirement(&r).unwrap();
    let locked = disk(&b, &h);
    let mut offline = h.clone();
    offline.title = "later title".into();
    offline.description = "offline body rewrite".into();
    offline.modified_at = locked.modified_at + chrono::Duration::minutes(1);
    for (a, z) in [(&locked, &offline), (&offline, &locked)] {
        let merged = merge_spec_three_way(&h, a, z);
        assert_eq!(merged.hypothesis_lock, locked.hypothesis_lock);
        assert_eq!(merged.description, h.description);
        assert_eq!(merged.title, offline.title);
        for resolution in [
            Resolution::AcceptLocal,
            Resolution::AcceptRemote,
            Resolution::LastWriteWins,
        ] {
            let merged = resolve_conflict(a, z, resolution);
            assert_eq!(merged.hypothesis_lock, locked.hypothesis_lock);
            assert_eq!(merged.description, h.description);
        }
    }
}

#[test]
fn hypothesis_lock_is_not_clearable_by_stale_save_or_deletion() {
    let (_tmp, b, h, mut r) = fixture();
    let stale = b.load().unwrap();
    r.status = RequirementStatus::Completed;
    b.update_requirement(&r).unwrap();
    let locked = disk(&b, &h);
    b.save(&stale).unwrap(); // untouched stale rows skip, never replace disk
    assert_eq!(disk(&b, &h).hypothesis_lock, locked.hypothesis_lock);
    let mut current = b.load().unwrap();
    let row = current
        .requirements
        .iter_mut()
        .find(|row| row.id == h.id)
        .unwrap();
    row.hypothesis_lock = None;
    row.tags.clear();
    b.save(&current).unwrap();
    assert_eq!(disk(&b, &h).hypothesis_lock, locked.hypothesis_lock);
    assert!(b
        .delete_requirement(&h.id)
        .unwrap_err()
        .to_string()
        .contains("aida comment add"));
    assert!(b
        .update_atomically(|s| s.requirements.retain(|row| row.id != h.id))
        .is_err());
    assert!(disk(&b, &h).hypothesis_lock.is_some());
}

#[test]
fn hypothesis_mismatched_persisted_hash_fails_closed() {
    let (_tmp, b, h, mut r) = fixture();
    r.status = RequirementStatus::Completed;
    b.update_requirement(&r).unwrap();
    let mut corrupted = disk(&b, &h);
    corrupted.description = "clean Git text merge rewrote this body".into();
    crate::object_store::write_object(&b.path().join("objects"), &corrupted).unwrap();
    corrupted.title = "ordinary edit must surface corrupt evidence".into();
    let err = b.update_requirement(&corrupted).unwrap_err();
    assert!(err
        .to_string()
        .contains("inconsistent locked hypothesis evidence"));
    assert_eq!(disk(&b, &h).title, h.title);
}

#[test]
fn hypothesis_importing_valid_existing_evidence_never_discards_the_lock() {
    let (_tmp, b, h, mut r) = fixture();
    r.status = RequirementStatus::Completed;
    b.update_requirement(&r).unwrap();
    let locked = disk(&b, &h);
    // A second canonical store last saw the row before the lock; import the
    // newer lock after the experiment is no longer linked in that snapshot.
    let (_tmp2, destination, original, _) = fixture();
    let mut imported = locked.clone();
    imported.id = original.id;
    imported.relationships.clear();
    imported.tags.clear();
    destination.update_requirement(&imported).unwrap();
    assert_eq!(
        disk(&destination, &original).hypothesis_lock,
        locked.hypothesis_lock
    );
    let mut malicious = imported.clone();
    malicious.hypothesis_lock = None;
    malicious.description = "discarding evidence must not unlock".into();
    assert!(destination.update_requirement(&malicious).is_err());
}

#[test]
fn hypothesis_atomic_update_cannot_erase_persisted_identity() {
    let (_tmp, b, h, mut r) = fixture();
    r.status = RequirementStatus::Completed;
    b.update_requirement(&r).unwrap();
    for replacement in [None, Some("SPIKE-9".into())] {
        let err = b
            .update_atomically(|s| {
                s.requirements
                    .iter_mut()
                    .find(|row| row.id == h.id)
                    .unwrap()
                    .spec_id = replacement;
            })
            .unwrap_err();
        assert!(err.to_string().contains("aida comment add"));
        assert!(disk(&b, &h).hypothesis_lock.is_some());
    }
}

#[test]
fn hypothesis_preflight_does_not_write_siblings_before_invalid_identity_or_evidence() {
    let (_tmp, b, h, mut r) = fixture();
    r.status = RequirementStatus::Completed;
    b.update_requirement(&r).unwrap();
    let before = disk(&b, &r);
    let mut sibling = before.clone();
    sibling.title = "must remain unmodified".into();
    let mut invalid = h.clone();
    invalid.spec_id = None;
    assert!(b
        .bulk_update(&[sibling.clone(), invalid], "bad input")
        .is_err());
    assert_eq!(disk(&b, &r).title, before.title);
    let path = crate::object_store::object_path(&b.path().join("objects"), "SPIKE-1").unwrap();
    std::fs::write(&path, "id: invalid-uuid\nhypothesis_lock: unreadable\n").unwrap();
    assert!(b.bulk_update(&[sibling, h], "unreadable evidence").is_err());
    assert_eq!(disk(&b, &r).title, before.title);
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "id: invalid-uuid\nhypothesis_lock: unreadable\n"
    );
}

#[test]
fn hypothesis_same_transaction_cannot_cancel_preregistration_at_result_boundary() {
    for rewrite in [false, true] {
        let (_tmp, b, h, r) = fixture();
        let result = b.update_atomically(|s| {
            for row in &mut s.requirements {
                if row.id == h.id {
                    row.tags.clear();
                    row.relationships.clear();
                    if rewrite {
                        row.description = "post hoc".into();
                    }
                }
                if row.id == r.id {
                    row.status = RequirementStatus::Completed;
                }
            }
        });
        if rewrite {
            assert!(result.unwrap_err().to_string().contains("aida comment add"));
            assert_eq!(disk(&b, &r).status, r.status);
        } else {
            result.unwrap();
            assert!(disk(&b, &h).hypothesis_lock.is_some());
        }
    }
    // A hypothesis may be withdrawn BEFORE a result exists.
    let (_tmp, b, mut h, mut r) = fixture();
    h.tags.clear();
    b.update_requirement(&h).unwrap();
    r.status = RequirementStatus::Completed;
    b.update_requirement(&r).unwrap();
    assert!(disk(&b, &h).hypothesis_lock.is_none());
}

#[test]
fn hypothesis_tree_and_validated_yaml_import_preserve_frozen_evidence() {
    let (_tmp, b, h, mut r) = fixture();
    r.status = RequirementStatus::Completed;
    b.update_requirement(&r).unwrap();
    let mut locked = disk(&b, &h);
    locked.tags.clear();
    locked.relationships.clear();
    b.update_requirement(&locked).unwrap();
    let source = b.load().unwrap();
    let tree = crate::export::export_tree(&source, "SPIKE-1").unwrap();
    let tree = serde_json::from_str(&serde_json::to_string(&tree).unwrap()).unwrap();
    let mut destination = RequirementsStore::new();
    crate::export::import_tree(&mut destination, tree, Default::default()).unwrap();
    assert_eq!(
        destination.requirements[0].hypothesis_lock,
        locked.hypothesis_lock
    );
    let tmp = tempfile::tempdir().unwrap();
    let new_backend = GitBackend::new(&tmp.path().join("tree"))
        .unwrap()
        .with_auto_commit(false);
    new_backend.save(&destination).unwrap();
    let mut imported = new_backend.load().unwrap().requirements.remove(0);
    imported.description = "post hoc after tree import".into();
    assert!(new_backend.update_requirement(&imported).is_err());

    let yaml = serde_yaml::to_string(&source).unwrap();
    let validated = crate::import::validate_import_content(&yaml).unwrap();
    let root = tmp.path().join("validated-yaml");
    let yaml_backend = GitBackend::new(&root).unwrap().with_auto_commit(false);
    let config = crate::import::ImportConfig {
        create_backup: false,
        ..Default::default()
    };
    // The validated-import converter produces a legacy YAML artifact; migrate
    // that artifact into the canonical backend before asserting enforcement.
    let artifact = tmp.path().join("imported.yaml");
    crate::import::execute_import(&yaml, &artifact, &config, &validated).unwrap();
    let converted: RequirementsStore =
        serde_yaml::from_str(&std::fs::read_to_string(artifact).unwrap()).unwrap();
    yaml_backend.save(&converted).unwrap();
    let mut imported = disk(&yaml_backend, &h);
    assert_eq!(imported.hypothesis_lock, locked.hypothesis_lock);
    imported.description = "post hoc after validated YAML import".into();
    assert!(yaml_backend.update_requirement(&imported).is_err());
}
