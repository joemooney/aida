//! Tests for ADR-55: System One EvaluatorEngine trait, JevEvaluator, and MockEvaluator.
//
// trace:ADR-55 | ai:antigravity

use crate::evaluator::{EvaluatorEngine, EvaluatorError, JevEvaluator, MockEvaluator};
use std::collections::HashMap;

#[test]
fn test_mock_evaluator_noul_success() {
    let mock = MockEvaluator::new().with_noul(0.98);
    let resp = mock
        .evaluate_noul_sync("test context", "test instruction")
        .unwrap();
    assert_eq!(resp.noul, 0.98);
    assert!(resp.heuristic);
    assert_eq!(resp.model, "mock-jev");
}

#[test]
fn test_mock_evaluator_choice_success() {
    let mut probs = HashMap::new();
    probs.insert("contradicts".to_string(), 0.88);
    probs.insert("compatible".to_string(), 0.12);

    let mock = MockEvaluator::new().with_choice("contradicts", 0.88, probs.clone());

    let mut options = HashMap::new();
    options.insert("contradicts".to_string(), "Specs conflict".to_string());
    options.insert("compatible".to_string(), "Specs agree".to_string());

    let resp = mock
        .evaluate_choice_sync("test context", "test instruction", &options)
        .unwrap();
    assert_eq!(resp.choice, "contradicts");
    assert_eq!(resp.confidence, 0.88);
    assert_eq!(resp.probabilities.get("contradicts"), Some(&0.88));
    assert!(resp.heuristic);
}

#[test]
fn test_mock_evaluator_score_success() {
    let mock = MockEvaluator::new().with_score(0.85);
    let levels = vec!["low".to_string(), "medium".to_string(), "high".to_string()];
    let resp = mock
        .evaluate_score_sync("test context", "test instruction", &levels)
        .unwrap();
    assert_eq!(resp.score, 0.85);
    assert!(resp.heuristic);
}

#[test]
fn test_mock_evaluator_fail_closed_error() {
    let mock =
        MockEvaluator::new().with_noul_error(EvaluatorError::Timeout("timed out".to_string()));
    let res = mock.evaluate_noul_sync("test context", "test instruction");
    assert!(res.is_err());
    match res.unwrap_err() {
        EvaluatorError::Timeout(msg) => assert_eq!(msg, "timed out"),
        other => panic!("expected timeout error, got {:?}", other),
    }
}

#[test]
fn test_jev_evaluator_builder() {
    let _jev = JevEvaluator::new("test-api-key")
        .with_endpoint("https://api.typesafe.ai/v1/systemone")
        .with_model("jev-1.13.0");
    assert_eq!(JevEvaluator::DEFAULT_MODEL, "jev-1.13.0");
}
