//! Unit and fault-injection tests for ADR-56 remote evaluator resilience,
//! deadline budgeting, circuit breaking, and structured telemetry.
//
// trace:ADR-56 trace:TASK-1430 trace:TASK-1431 trace:TASK-1432 | ai:antigravity

use crate::evaluator::{EvaluatorEngine, EvaluatorError, EvaluatorErrorKind, MockEvaluator};
use crate::evaluator_resilience::{
    compute_idempotency_key, CircuitBreakerConfig, CircuitState, DeadlineRetryAdapter,
    EvaluatorCircuitBreaker, InMemoryTelemetrySink, RetryPolicy,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn test_circuit_breaker_initial_state_closed() {
    let cb = EvaluatorCircuitBreaker::new("test-endpoint", CircuitBreakerConfig::default());
    assert_eq!(cb.state(), CircuitState::Closed);
    assert_eq!(cb.consecutive_failures(), 0);
    assert!(cb.check_permitted().is_ok());
}

#[test]
fn test_circuit_breaker_permanent_errors_do_not_trip() {
    let cb = EvaluatorCircuitBreaker::new("test-endpoint", CircuitBreakerConfig::default());

    let permanent_errors = vec![
        EvaluatorError::MissingApiKey("no key".to_string()),
        EvaluatorError::ApiError {
            status: 400,
            message: "Bad Request".to_string(),
        },
        EvaluatorError::ApiError {
            status: 401,
            message: "Unauthorized".to_string(),
        },
        EvaluatorError::ApiError {
            status: 403,
            message: "Forbidden".to_string(),
        },
        EvaluatorError::ApiError {
            status: 404,
            message: "Not Found".to_string(),
        },
        EvaluatorError::ParseError("invalid json schema".to_string()),
        EvaluatorError::Other("unsupported configuration".to_string()),
    ];

    for err in permanent_errors {
        assert_eq!(err.classify(), EvaluatorErrorKind::Permanent);
        cb.record_failure(&err);
        assert_eq!(
            cb.consecutive_failures(),
            0,
            "permanent error {:?} incremented failure count",
            err
        );
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.check_permitted().is_ok());
    }
}

#[test]
fn test_circuit_breaker_transient_failures_trip_open() {
    let config = CircuitBreakerConfig {
        failure_threshold: 3,
        cooldown_duration: Duration::from_secs(60),
    };
    let cb = EvaluatorCircuitBreaker::new("test-endpoint", config);

    let transient_err1 = EvaluatorError::Network("connection reset by peer".to_string());
    assert_eq!(transient_err1.classify(), EvaluatorErrorKind::Transient);
    cb.record_failure(&transient_err1);
    assert_eq!(cb.consecutive_failures(), 1);
    assert_eq!(cb.state(), CircuitState::Closed);

    let transient_err2 = EvaluatorError::Timeout("request timed out".to_string());
    cb.record_failure(&transient_err2);
    assert_eq!(cb.consecutive_failures(), 2);
    assert_eq!(cb.state(), CircuitState::Closed);

    let transient_err3 = EvaluatorError::ApiError {
        status: 503,
        message: "Service Unavailable".to_string(),
    };
    cb.record_failure(&transient_err3);
    assert_eq!(cb.consecutive_failures(), 3);
    assert_eq!(cb.state(), CircuitState::Open);

    // After tripping open, check_permitted must fail fast closed immediately
    let check = cb.check_permitted();
    assert!(check.is_err());
    match check.unwrap_err() {
        EvaluatorError::CircuitOpen { endpoint } => assert_eq!(endpoint, "test-endpoint"),
        other => panic!("expected CircuitOpen, got {:?}", other),
    }
}

#[test]
fn test_circuit_breaker_cooldown_and_half_open_probe_success() {
    let config = CircuitBreakerConfig {
        failure_threshold: 3,
        cooldown_duration: Duration::from_millis(30),
    };
    let cb = EvaluatorCircuitBreaker::new("test-endpoint", config);

    // Trip circuit breaker
    let err = EvaluatorError::Network("DNS flap".to_string());
    cb.record_failure(&err);
    cb.record_failure(&err);
    cb.record_failure(&err);
    assert_eq!(cb.state(), CircuitState::Open);

    // Wait for cooldown to expire
    std::thread::sleep(Duration::from_millis(40));

    // First call after cooldown should transition to HalfOpen and be permitted
    assert!(cb.check_permitted().is_ok());
    assert_eq!(cb.state(), CircuitState::HalfOpen);

    // Concurrency guard: second probe during HalfOpen is rejected
    assert!(cb.check_permitted().is_err());

    // Successful probe recovers the breaker to Closed
    cb.record_success();
    assert_eq!(cb.state(), CircuitState::Closed);
    assert_eq!(cb.consecutive_failures(), 0);
    assert!(cb.check_permitted().is_ok());
}

#[test]
fn test_circuit_breaker_half_open_probe_failure_re_trips_open() {
    let config = CircuitBreakerConfig {
        failure_threshold: 3,
        cooldown_duration: Duration::from_millis(30),
    };
    let cb = EvaluatorCircuitBreaker::new("test-endpoint", config);

    // Trip circuit breaker
    let err = EvaluatorError::Network("timeout".to_string());
    cb.record_failure(&err);
    cb.record_failure(&err);
    cb.record_failure(&err);
    assert_eq!(cb.state(), CircuitState::Open);

    std::thread::sleep(Duration::from_millis(40));

    // Allow 1 probe in half-open
    assert!(cb.check_permitted().is_ok());
    assert_eq!(cb.state(), CircuitState::HalfOpen);

    // Probe fails with transient error
    cb.record_failure(&EvaluatorError::ApiError {
        status: 502,
        message: "Bad Gateway".to_string(),
    });

    // Re-trips immediately back to Open
    assert_eq!(cb.state(), CircuitState::Open);
    assert!(cb.check_permitted().is_err());
}

#[test]
fn test_deadline_retry_adapter_success_first_attempt() {
    let mock = MockEvaluator::new().with_noul(0.95);
    let cb = Arc::new(EvaluatorCircuitBreaker::new(
        "test-endpoint",
        CircuitBreakerConfig::default(),
    ));
    let sink = Arc::new(InMemoryTelemetrySink::new());

    let adapter = DeadlineRetryAdapter::new(
        mock,
        "test-endpoint",
        cb.clone(),
        RetryPolicy::default(),
        Duration::from_secs(5),
    )
    .with_telemetry(sink.clone());

    let res = adapter
        .evaluate_noul_sync("context", "instruction")
        .unwrap();
    assert_eq!(res.noul, 0.95);
    assert_eq!(cb.consecutive_failures(), 0);

    let events = sink.events();
    assert!(!events.is_empty());
    assert_eq!(sink.count_retries(), 0);
}

#[test]
fn test_deadline_retry_adapter_transient_retry_success() {
    let mock = MockEvaluator::new()
        .with_noul_error(EvaluatorError::Network("connection reset".to_string()))
        .with_noul(0.99);

    let cb = Arc::new(EvaluatorCircuitBreaker::new(
        "test-endpoint",
        CircuitBreakerConfig::default(),
    ));
    let sink = Arc::new(InMemoryTelemetrySink::new());

    let policy = RetryPolicy {
        max_retries: 1,
        min_budget_for_retry: Duration::from_millis(100),
        initial_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(50),
    };

    let adapter = DeadlineRetryAdapter::new(
        mock,
        "test-endpoint",
        cb.clone(),
        policy,
        Duration::from_secs(2),
    )
    .with_telemetry(sink.clone());

    let res = adapter
        .evaluate_noul_sync("context", "instruction")
        .unwrap();
    assert_eq!(res.noul, 0.99);

    // Verify retry was recorded in telemetry
    assert_eq!(sink.count_retries(), 1);
    // Failure counter is reset on success
    assert_eq!(cb.consecutive_failures(), 0);
    assert_eq!(cb.state(), CircuitState::Closed);
}

#[test]
fn test_deadline_retry_adapter_permanent_fails_immediately_without_retry() {
    let mock = MockEvaluator::new().with_noul_error(EvaluatorError::ApiError {
        status: 401,
        message: "Unauthorized - bad api key".to_string(),
    });

    let cb = Arc::new(EvaluatorCircuitBreaker::new(
        "test-endpoint",
        CircuitBreakerConfig::default(),
    ));
    let sink = Arc::new(InMemoryTelemetrySink::new());

    let policy = RetryPolicy {
        max_retries: 2,
        min_budget_for_retry: Duration::from_millis(100),
        initial_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(50),
    };

    let adapter = DeadlineRetryAdapter::new(
        mock,
        "test-endpoint",
        cb.clone(),
        policy,
        Duration::from_secs(5),
    )
    .with_telemetry(sink.clone());

    let res = adapter.evaluate_noul_sync("context", "instruction");
    assert!(res.is_err());
    match res.unwrap_err() {
        EvaluatorError::ApiError { status, .. } => assert_eq!(status, 401),
        other => panic!("expected 401 ApiError, got {:?}", other),
    }

    // Must NOT have attempted any retries
    assert_eq!(sink.count_retries(), 0);
    // Must NOT have incremented failure counter
    assert_eq!(cb.consecutive_failures(), 0);
    assert_eq!(cb.state(), CircuitState::Closed);
}

#[test]
fn test_deadline_retry_adapter_budget_exhaustion_aborts_retry() {
    let mock =
        MockEvaluator::new().with_noul_error(EvaluatorError::Network("slow flap".to_string()));

    let cb = Arc::new(EvaluatorCircuitBreaker::new(
        "test-endpoint",
        CircuitBreakerConfig::default(),
    ));
    let sink = Arc::new(InMemoryTelemetrySink::new());

    // Deadline is 20ms, but min_budget_for_retry is 500ms
    let policy = RetryPolicy {
        max_retries: 2,
        min_budget_for_retry: Duration::from_millis(500),
        initial_backoff: Duration::from_millis(50),
        max_backoff: Duration::from_millis(100),
    };

    let adapter = DeadlineRetryAdapter::new(
        mock,
        "test-endpoint",
        cb.clone(),
        policy,
        Duration::from_millis(20),
    )
    .with_telemetry(sink.clone());

    let res = adapter.evaluate_noul_sync("context", "instruction");
    assert!(res.is_err());
    assert_eq!(res.unwrap_err(), EvaluatorError::DeadlineExceeded);

    // Aborted without retrying
    assert_eq!(sink.count_retries(), 0);
}

#[test]
fn test_idempotency_key_deterministic_and_unique() {
    let key1 = compute_idempotency_key("noul", "context A", "instruction 1");
    let key2 = compute_idempotency_key("noul", "context A", "instruction 1");
    let key3 = compute_idempotency_key("noul", "context A", "instruction 2");
    let key4 = compute_idempotency_key("choice", "context A", "instruction 1");

    assert_eq!(key1, key2, "identical inputs must yield identical hash");
    assert_ne!(
        key1, key3,
        "different instruction must yield different hash"
    );
    assert_ne!(key1, key4, "different operation must yield different hash");
}

#[test]
fn test_choice_and_score_evaluation_through_resilience_adapter() {
    let mut probs = HashMap::new();
    probs.insert("approve".to_string(), 0.95);
    probs.insert("reject".to_string(), 0.05);

    let mock = MockEvaluator::new()
        .with_choice("approve", 0.95, probs)
        .with_score(0.88);

    let cb = Arc::new(EvaluatorCircuitBreaker::new(
        "test-endpoint",
        CircuitBreakerConfig::default(),
    ));
    let adapter = DeadlineRetryAdapter::new(
        mock,
        "test-endpoint",
        cb,
        RetryPolicy::default(),
        Duration::from_secs(5),
    );

    let mut options = HashMap::new();
    options.insert("approve".to_string(), "Approve change".to_string());
    options.insert("reject".to_string(), "Reject change".to_string());

    let choice_resp = adapter
        .evaluate_choice_sync("ctx", "inst", &options)
        .unwrap();
    assert_eq!(choice_resp.choice, "approve");

    let levels = vec!["low".to_string(), "high".to_string()];
    let score_resp = adapter.evaluate_score_sync("ctx", "inst", &levels).unwrap();
    assert_eq!(score_resp.score, 0.88);
}
