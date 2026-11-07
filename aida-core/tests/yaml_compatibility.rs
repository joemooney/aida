// trace:TASK-3325 | ai:codex
use serde::{Deserialize, Serialize};

#[test]
fn stored_requirement_round_trips_without_model_changes() {
    let mut requirement =
        aida_core::Requirement::new("YAML compatibility".into(), "& * ! prose".into());
    requirement.spec_id = Some("FR-0042".into());
    let before = serde_json::to_value(&requirement).unwrap();
    let yaml = serde_yaml::to_string(&requirement).unwrap();
    let restored: aida_core::Requirement = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(serde_json::to_value(&restored).unwrap(), before);
    assert_eq!(serde_yaml::to_string(&restored).unwrap(), yaml);
}

#[test]
fn legacy_yaml_scalars_and_tags_keep_their_meaning() {
    let legacy = "text: 'true'\nempty: null\nflag: false\nlarge: 18446744073709551615\nkind: !Custom payload\nprose: |\n  &anchor-looking *alias-looking !tag-looking\n";
    let value: serde_yaml::Value = serde_yaml::from_str(legacy).unwrap();
    assert_eq!(value["text"].as_str(), Some("true"));
    assert!(value["empty"].is_null());
    assert_eq!(value["flag"].as_bool(), Some(false));
    assert_eq!(value["large"].as_u64(), Some(u64::MAX));
    assert!(matches!(&value["kind"], serde_yaml::Value::Tagged(_)));
    let serialized = serde_yaml::to_string(&value).unwrap();
    assert_eq!(
        serde_yaml::from_str::<serde_yaml::Value>(&serialized).unwrap(),
        value
    );
}

#[test]
fn native_enum_tag_encoding_is_unchanged() {
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum Operation {
        Create { title: String },
    }
    let legacy = "!Create\ntitle: Example\n";
    let operation: Operation = serde_yaml::from_str(legacy).unwrap();
    assert_eq!(
        operation,
        Operation::Create {
            title: "Example".into()
        }
    );
    assert_eq!(serde_yaml::to_string(&operation).unwrap(), legacy);
}

#[test]
fn duplicate_mapping_keys_are_rejected() {
    assert!(serde_yaml::from_str::<serde_yaml::Value>("title: first\ntitle: second\n").is_err());
}

/// Captured using the original serde_yaml 0.9.34 and unchanged Requirement
/// model. Singleton maps/sets make field ordering deterministic. This checks
/// quoting, multiline Unicode prose, timestamps, enums and omitted defaults.
#[test]
fn stored_requirement_encoding_matches_legacy_bytes() {
    let legacy = include_str!("fixtures/requirement-serde-yaml-0.9.yaml");
    let requirement: aida_core::Requirement = serde_yaml::from_str(legacy).unwrap();
    assert_eq!(serde_yaml::to_string(&requirement).unwrap(), legacy);
}
