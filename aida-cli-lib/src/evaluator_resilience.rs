//! Remote evaluator transport resilience, failure classification, deadline budgeting,
//! concurrency-safe circuit breaking, and structured telemetry (ADR-56).
//
// trace:ADR-56 trace:TASK-1430 trace:TASK-1431 trace:TASK-1432 trace:TASK-1433 | ai:antigravity

use crate::evaluator::{
    ChoiceResponse, EvaluatorEngine, EvaluatorError, EvaluatorErrorKind, EvaluatorFuture,
    NoulResponse, ScoreResponse,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Concurrency-safe circuit state (ADR-56, TASK-1431).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CircuitState {
    /// Normal operation: requests pass through.
    Closed,
    /// Outage detected: requests fail closed fast without network IO.
    Open,
    /// Recovery probing: exactly 1 trial probe allowed to test service recovery.
    HalfOpen,
}

impl std::fmt::Display for CircuitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(f, "closed"),
            Self::Open => write!(f, "open"),
            Self::HalfOpen => write!(f, "half-open"),
        }
    }
}

/// Configuration for the evaluator circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive transient failures before opening the circuit (default 3).
    pub failure_threshold: usize,
    /// Duration the circuit stays open before permitting a half-open trial probe (default 60s).
    pub cooldown_duration: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            cooldown_duration: Duration::from_secs(60),
        }
    }
}

/// Inner state guarded by mutex for thread safety across concurrent agent workers.
#[derive(Debug)]
struct CircuitBreakerInner {
    state: CircuitState,
    consecutive_transient_failures: usize,
    tripped_at: Option<Instant>,
    half_open_in_flight: bool,
}

/// Concurrency-safe, endpoint/model-scoped circuit breaker (TASK-1431).
// trace:ADR-56 trace:TASK-1431 | ai:antigravity
#[derive(Debug)]
pub struct EvaluatorCircuitBreaker {
    endpoint: String,
    config: CircuitBreakerConfig,
    inner: Mutex<CircuitBreakerInner>,
    telemetry: Option<Arc<dyn TelemetrySink>>,
}

impl EvaluatorCircuitBreaker {
    /// Creates a new circuit breaker for an endpoint with default or custom configuration.
    pub fn new(endpoint: impl Into<String>, config: CircuitBreakerConfig) -> Self {
        Self {
            endpoint: endpoint.into(),
            config,
            inner: Mutex::new(CircuitBreakerInner {
                state: CircuitState::Closed,
                consecutive_transient_failures: 0,
                tripped_at: None,
                half_open_in_flight: false,
            }),
            telemetry: None,
        }
    }

    /// Attaches a telemetry sink for structured event logging.
    pub fn with_telemetry(mut self, sink: Arc<dyn TelemetrySink>) -> Self {
        self.telemetry = Some(sink);
        self
    }

    /// Returns the endpoint name this breaker protects.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Returns the current circuit state.
    pub fn state(&self) -> CircuitState {
        let mut inner = self.inner.lock().unwrap();
        self.check_state_transition(&mut inner);
        inner.state
    }

    /// Returns the count of consecutive transient failures.
    pub fn consecutive_failures(&self) -> usize {
        self.inner.lock().unwrap().consecutive_transient_failures
    }

    /// Checks if a request is permitted to proceed.
    /// Returns `Ok(())` if permitted, or `Err(EvaluatorError::CircuitOpen)` if the circuit is open.
    pub fn check_permitted(&self) -> Result<(), EvaluatorError> {
        let mut inner = self.inner.lock().unwrap();
        self.check_state_transition(&mut inner);

        match inner.state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open => Err(EvaluatorError::CircuitOpen {
                endpoint: self.endpoint.clone(),
            }),
            CircuitState::HalfOpen => {
                if inner.half_open_in_flight {
                    // Only 1 trial request in flight during half-open
                    Err(EvaluatorError::CircuitOpen {
                        endpoint: self.endpoint.clone(),
                    })
                } else {
                    inner.half_open_in_flight = true;
                    Ok(())
                }
            }
        }
    }

    /// Internal transition check: if Open and cooldown elapsed, move to HalfOpen.
    fn check_state_transition(&self, inner: &mut CircuitBreakerInner) {
        if inner.state == CircuitState::Open {
            if let Some(tripped) = inner.tripped_at {
                if tripped.elapsed() >= self.config.cooldown_duration {
                    let old_state = inner.state;
                    inner.state = CircuitState::HalfOpen;
                    inner.half_open_in_flight = false;
                    self.emit_telemetry(EvaluatorTelemetryEvent::CircuitStateChange {
                        endpoint: self.endpoint.clone(),
                        from: old_state,
                        to: CircuitState::HalfOpen,
                        reason: format!(
                            "Cooldown elapsed ({:?}); allowing half-open probe",
                            self.config.cooldown_duration
                        ),
                        timestamp: Utc::now().to_rfc3339(),
                    });
                }
            }
        }
    }

    /// Records a successful evaluation: resets failures and closes the circuit if half-open.
    pub fn record_success(&self) {
        let mut inner = self.inner.lock().unwrap();
        let old_state = inner.state;
        inner.consecutive_transient_failures = 0;
        inner.tripped_at = None;
        inner.half_open_in_flight = false;

        if old_state != CircuitState::Closed {
            inner.state = CircuitState::Closed;
            self.emit_telemetry(EvaluatorTelemetryEvent::CircuitStateChange {
                endpoint: self.endpoint.clone(),
                from: old_state,
                to: CircuitState::Closed,
                reason: "Probe succeeded; circuit recovered to closed".to_string(),
                timestamp: Utc::now().to_rfc3339(),
            });
        }
    }

    /// Records an evaluation failure.
    /// Permanent errors (400, 401 Unauthorized, 403 Forbidden, 404, schema errors) fail closed
    /// immediately and MUST NOT increment transient failure counters or trip the breaker.
    pub fn record_failure(&self, error: &EvaluatorError) {
        let mut inner = self.inner.lock().unwrap();

        match error.classify() {
            EvaluatorErrorKind::Permanent
            | EvaluatorErrorKind::DeadlineExceeded
            | EvaluatorErrorKind::CircuitOpen => {
                // Invariant: Permanent or deadline errors do NOT increment transient failure count.
                // Reset half_open_in_flight if in HalfOpen so subsequent calls can still attempt.
                if inner.state == CircuitState::HalfOpen {
                    inner.half_open_in_flight = false;
                }
            }
            EvaluatorErrorKind::Transient => {
                match inner.state {
                    CircuitState::HalfOpen => {
                        // Trial probe failed! Trip back to Open with fresh cooldown.
                        let old_state = inner.state;
                        inner.state = CircuitState::Open;
                        inner.tripped_at = Some(Instant::now());
                        inner.half_open_in_flight = false;
                        self.emit_telemetry(EvaluatorTelemetryEvent::CircuitStateChange {
                            endpoint: self.endpoint.clone(),
                            from: old_state,
                            to: CircuitState::Open,
                            reason: format!(
                                "Half-open trial probe failed with transient error: {error}"
                            ),
                            timestamp: Utc::now().to_rfc3339(),
                        });
                    }
                    CircuitState::Closed => {
                        inner.consecutive_transient_failures += 1;
                        if inner.consecutive_transient_failures >= self.config.failure_threshold {
                            let old_state = inner.state;
                            inner.state = CircuitState::Open;
                            inner.tripped_at = Some(Instant::now());
                            self.emit_telemetry(EvaluatorTelemetryEvent::CircuitStateChange {
                                endpoint: self.endpoint.clone(),
                                from: old_state,
                                to: CircuitState::Open,
                                reason: format!(
                                    "Consecutive transient failures reached threshold ({}) with: {error}",
                                    self.config.failure_threshold
                                ),
                                timestamp: Utc::now().to_rfc3339(),
                            });
                        }
                    }
                    CircuitState::Open => {
                        // Already open; do nothing.
                    }
                }
            }
        }
    }

    fn emit_telemetry(&self, event: EvaluatorTelemetryEvent) {
        if let Some(sink) = &self.telemetry {
            sink.emit(event);
        }
    }
}

/// Retry policy with budget awareness and backoff limits (TASK-1430).
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum retry attempts (default 1, max 2).
    pub max_retries: usize,
    /// Minimum remaining deadline budget required to attempt a retry (default 500ms).
    pub min_budget_for_retry: Duration,
    /// Initial backoff duration before first retry (default 100ms).
    pub initial_backoff: Duration,
    /// Maximum backoff duration cap (default 1000ms).
    pub max_backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 1,
            min_budget_for_retry: Duration::from_millis(500),
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_millis(1000),
        }
    }
}

impl RetryPolicy {
    /// Calculates exponential backoff for a zero-indexed retry attempt.
    pub fn backoff_for_attempt(&self, attempt: usize) -> Duration {
        let mult = 2u32.saturating_pow(attempt as u32);
        let backoff = self.initial_backoff.saturating_mul(mult);
        backoff.min(self.max_backoff)
    }
}

/// Computes an idempotency key from evaluation inputs to prevent duplicate billing charges (TASK-1430).
pub fn compute_idempotency_key(operation: &str, context: &str, instruction: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(operation.as_bytes());
    hasher.update(b"|");
    hasher.update(context.as_bytes());
    hasher.update(b"|");
    hasher.update(instruction.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Generic deadline-aware async retry adapter wrapping any `EvaluatorEngine` (TASK-1430).
// trace:ADR-56 trace:TASK-1430 trace:TASK-1433 | ai:antigravity
pub struct DeadlineRetryAdapter<E: EvaluatorEngine> {
    inner: E,
    circuit_breaker: Arc<EvaluatorCircuitBreaker>,
    policy: RetryPolicy,
    default_deadline: Duration,
    endpoint: String,
    telemetry: Option<Arc<dyn TelemetrySink>>,
}

impl<E: EvaluatorEngine> DeadlineRetryAdapter<E> {
    /// Constructs a new `DeadlineRetryAdapter`.
    pub fn new(
        inner: E,
        endpoint: impl Into<String>,
        circuit_breaker: Arc<EvaluatorCircuitBreaker>,
        policy: RetryPolicy,
        default_deadline: Duration,
    ) -> Self {
        Self {
            inner,
            endpoint: endpoint.into(),
            circuit_breaker,
            policy,
            default_deadline,
            telemetry: None,
        }
    }

    /// Attaches a telemetry sink.
    pub fn with_telemetry(mut self, sink: Arc<dyn TelemetrySink>) -> Self {
        self.telemetry = Some(sink);
        self
    }

    /// Returns a reference to the inner evaluator.
    pub fn inner(&self) -> &E {
        &self.inner
    }

    /// Returns a reference to the circuit breaker.
    pub fn circuit_breaker(&self) -> &Arc<EvaluatorCircuitBreaker> {
        &self.circuit_breaker
    }

    fn emit_telemetry(&self, event: EvaluatorTelemetryEvent) {
        if let Some(sink) = &self.telemetry {
            sink.emit(event);
        }
    }
}

impl<E: EvaluatorEngine + 'static> EvaluatorEngine for DeadlineRetryAdapter<E> {
    fn evaluate_noul<'a>(
        &'a self,
        context: &'a str,
        instruction: &'a str,
    ) -> EvaluatorFuture<'a, NoulResponse> {
        Box::pin(async move {
            let start = Instant::now();
            let deadline = start + self.default_deadline;
            let idempotency_key = compute_idempotency_key("noul", context, instruction);

            let mut attempt = 0usize;
            loop {
                // 1. Circuit breaker gate
                if let Err(err) = self.circuit_breaker.check_permitted() {
                    self.emit_telemetry(EvaluatorTelemetryEvent::Fallback {
                        endpoint: self.endpoint.clone(),
                        operation: "noul".to_string(),
                        reason: "Circuit open (fail-closed fallback)".to_string(),
                        duration_ms: start.elapsed().as_millis() as u64,
                        timestamp: Utc::now().to_rfc3339(),
                    });
                    return Err(err);
                }

                // 2. Budget check
                let now = Instant::now();
                if now >= deadline {
                    let err = EvaluatorError::DeadlineExceeded;
                    self.circuit_breaker.record_failure(&err);
                    self.emit_telemetry(EvaluatorTelemetryEvent::Fallback {
                        endpoint: self.endpoint.clone(),
                        operation: "noul".to_string(),
                        reason: "Operation deadline budget exhausted".to_string(),
                        duration_ms: start.elapsed().as_millis() as u64,
                        timestamp: Utc::now().to_rfc3339(),
                    });
                    return Err(err);
                }
                let remaining = deadline - now;
                if attempt > 0 && remaining < self.policy.min_budget_for_retry {
                    let err = EvaluatorError::DeadlineExceeded;
                    self.circuit_breaker.record_failure(&err);
                    self.emit_telemetry(EvaluatorTelemetryEvent::Fallback {
                        endpoint: self.endpoint.clone(),
                        operation: "noul".to_string(),
                        reason: format!(
                            "Remaining budget ({:?}) below minimum retry threshold ({:?})",
                            remaining, self.policy.min_budget_for_retry
                        ),
                        duration_ms: start.elapsed().as_millis() as u64,
                        timestamp: Utc::now().to_rfc3339(),
                    });
                    return Err(err);
                }

                // 3. Emit attempt telemetry
                self.emit_telemetry(EvaluatorTelemetryEvent::Attempt {
                    endpoint: self.endpoint.clone(),
                    operation: "noul".to_string(),
                    attempt,
                    remaining_budget_ms: remaining.as_millis() as u64,
                    idempotency_key: idempotency_key.clone(),
                    timestamp: Utc::now().to_rfc3339(),
                });

                // 4. Execute call
                match self.inner.evaluate_noul(context, instruction).await {
                    Ok(resp) => {
                        self.circuit_breaker.record_success();
                        return Ok(resp);
                    }
                    Err(err) => {
                        self.circuit_breaker.record_failure(&err);

                        // Permanent errors fail closed immediately without retry
                        if err.classify() != EvaluatorErrorKind::Transient {
                            self.emit_telemetry(EvaluatorTelemetryEvent::Fallback {
                                endpoint: self.endpoint.clone(),
                                operation: "noul".to_string(),
                                reason: format!(
                                    "Permanent failure ({:?}): {}",
                                    err.classify(),
                                    err
                                ),
                                duration_ms: start.elapsed().as_millis() as u64,
                                timestamp: Utc::now().to_rfc3339(),
                            });
                            return Err(err);
                        }

                        // Max retries check
                        if attempt >= self.policy.max_retries {
                            self.emit_telemetry(EvaluatorTelemetryEvent::Fallback {
                                endpoint: self.endpoint.clone(),
                                operation: "noul".to_string(),
                                reason: format!(
                                    "Retries exhausted ({attempt}/{}): {err}",
                                    self.policy.max_retries
                                ),
                                duration_ms: start.elapsed().as_millis() as u64,
                                timestamp: Utc::now().to_rfc3339(),
                            });
                            return Err(err);
                        }

                        // Backoff calculation
                        let backoff = self.policy.backoff_for_attempt(attempt);
                        let post_backoff_now = Instant::now() + backoff;
                        if post_backoff_now >= deadline
                            || (deadline - post_backoff_now) < self.policy.min_budget_for_retry
                        {
                            let budget_err = EvaluatorError::DeadlineExceeded;
                            self.circuit_breaker.record_failure(&budget_err);
                            self.emit_telemetry(EvaluatorTelemetryEvent::Fallback {
                                endpoint: self.endpoint.clone(),
                                operation: "noul".to_string(),
                                reason: "Backoff duration exceeds remaining deadline budget"
                                    .to_string(),
                                duration_ms: start.elapsed().as_millis() as u64,
                                timestamp: Utc::now().to_rfc3339(),
                            });
                            return Err(budget_err);
                        }

                        self.emit_telemetry(EvaluatorTelemetryEvent::Retry {
                            endpoint: self.endpoint.clone(),
                            operation: "noul".to_string(),
                            attempt,
                            error_kind: err.classify(),
                            error_message: err.to_string(),
                            backoff_ms: backoff.as_millis() as u64,
                            timestamp: Utc::now().to_rfc3339(),
                        });

                        tokio::time::sleep(backoff).await;
                        attempt += 1;
                    }
                }
            }
        })
    }

    fn evaluate_choice<'a>(
        &'a self,
        context: &'a str,
        instruction: &'a str,
        options: &'a HashMap<String, String>,
    ) -> EvaluatorFuture<'a, ChoiceResponse> {
        Box::pin(async move {
            let start = Instant::now();
            let deadline = start + self.default_deadline;
            let idempotency_key = compute_idempotency_key("choice", context, instruction);

            let mut attempt = 0usize;
            loop {
                if let Err(err) = self.circuit_breaker.check_permitted() {
                    self.emit_telemetry(EvaluatorTelemetryEvent::Fallback {
                        endpoint: self.endpoint.clone(),
                        operation: "choice".to_string(),
                        reason: "Circuit open (fail-closed fallback)".to_string(),
                        duration_ms: start.elapsed().as_millis() as u64,
                        timestamp: Utc::now().to_rfc3339(),
                    });
                    return Err(err);
                }

                let now = Instant::now();
                if now >= deadline {
                    let err = EvaluatorError::DeadlineExceeded;
                    self.circuit_breaker.record_failure(&err);
                    return Err(err);
                }
                let remaining = deadline - now;
                if attempt > 0 && remaining < self.policy.min_budget_for_retry {
                    let err = EvaluatorError::DeadlineExceeded;
                    self.circuit_breaker.record_failure(&err);
                    return Err(err);
                }

                self.emit_telemetry(EvaluatorTelemetryEvent::Attempt {
                    endpoint: self.endpoint.clone(),
                    operation: "choice".to_string(),
                    attempt,
                    remaining_budget_ms: remaining.as_millis() as u64,
                    idempotency_key: idempotency_key.clone(),
                    timestamp: Utc::now().to_rfc3339(),
                });

                match self
                    .inner
                    .evaluate_choice(context, instruction, options)
                    .await
                {
                    Ok(resp) => {
                        self.circuit_breaker.record_success();
                        return Ok(resp);
                    }
                    Err(err) => {
                        self.circuit_breaker.record_failure(&err);
                        if err.classify() != EvaluatorErrorKind::Transient {
                            return Err(err);
                        }
                        if attempt >= self.policy.max_retries {
                            return Err(err);
                        }

                        let backoff = self.policy.backoff_for_attempt(attempt);
                        let post_backoff_now = Instant::now() + backoff;
                        if post_backoff_now >= deadline
                            || (deadline - post_backoff_now) < self.policy.min_budget_for_retry
                        {
                            return Err(EvaluatorError::DeadlineExceeded);
                        }

                        self.emit_telemetry(EvaluatorTelemetryEvent::Retry {
                            endpoint: self.endpoint.clone(),
                            operation: "choice".to_string(),
                            attempt,
                            error_kind: err.classify(),
                            error_message: err.to_string(),
                            backoff_ms: backoff.as_millis() as u64,
                            timestamp: Utc::now().to_rfc3339(),
                        });

                        tokio::time::sleep(backoff).await;
                        attempt += 1;
                    }
                }
            }
        })
    }

    fn evaluate_score<'a>(
        &'a self,
        context: &'a str,
        instruction: &'a str,
        levels: &'a [String],
    ) -> EvaluatorFuture<'a, ScoreResponse> {
        Box::pin(async move {
            let start = Instant::now();
            let deadline = start + self.default_deadline;
            let idempotency_key = compute_idempotency_key("score", context, instruction);

            let mut attempt = 0usize;
            loop {
                if let Err(err) = self.circuit_breaker.check_permitted() {
                    return Err(err);
                }

                let now = Instant::now();
                if now >= deadline {
                    let err = EvaluatorError::DeadlineExceeded;
                    self.circuit_breaker.record_failure(&err);
                    return Err(err);
                }
                let remaining = deadline - now;
                if attempt > 0 && remaining < self.policy.min_budget_for_retry {
                    let err = EvaluatorError::DeadlineExceeded;
                    self.circuit_breaker.record_failure(&err);
                    return Err(err);
                }

                self.emit_telemetry(EvaluatorTelemetryEvent::Attempt {
                    endpoint: self.endpoint.clone(),
                    operation: "score".to_string(),
                    attempt,
                    remaining_budget_ms: remaining.as_millis() as u64,
                    idempotency_key: idempotency_key.clone(),
                    timestamp: Utc::now().to_rfc3339(),
                });

                match self
                    .inner
                    .evaluate_score(context, instruction, levels)
                    .await
                {
                    Ok(resp) => {
                        self.circuit_breaker.record_success();
                        return Ok(resp);
                    }
                    Err(err) => {
                        self.circuit_breaker.record_failure(&err);
                        if err.classify() != EvaluatorErrorKind::Transient {
                            return Err(err);
                        }
                        if attempt >= self.policy.max_retries {
                            return Err(err);
                        }

                        let backoff = self.policy.backoff_for_attempt(attempt);
                        let post_backoff_now = Instant::now() + backoff;
                        if post_backoff_now >= deadline
                            || (deadline - post_backoff_now) < self.policy.min_budget_for_retry
                        {
                            return Err(EvaluatorError::DeadlineExceeded);
                        }

                        self.emit_telemetry(EvaluatorTelemetryEvent::Retry {
                            endpoint: self.endpoint.clone(),
                            operation: "score".to_string(),
                            attempt,
                            error_kind: err.classify(),
                            error_message: err.to_string(),
                            backoff_ms: backoff.as_millis() as u64,
                            timestamp: Utc::now().to_rfc3339(),
                        });

                        tokio::time::sleep(backoff).await;
                        attempt += 1;
                    }
                }
            }
        })
    }
}

/// Structured telemetry event for observability (TASK-1432).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event_type")]
pub enum EvaluatorTelemetryEvent {
    Attempt {
        endpoint: String,
        operation: String,
        attempt: usize,
        remaining_budget_ms: u64,
        idempotency_key: String,
        timestamp: String,
    },
    Retry {
        endpoint: String,
        operation: String,
        attempt: usize,
        error_kind: EvaluatorErrorKind,
        error_message: String,
        backoff_ms: u64,
        timestamp: String,
    },
    CircuitStateChange {
        endpoint: String,
        from: CircuitState,
        to: CircuitState,
        reason: String,
        timestamp: String,
    },
    Fallback {
        endpoint: String,
        operation: String,
        reason: String,
        duration_ms: u64,
        timestamp: String,
    },
}

/// Telemetry sink trait allowing in-memory capture for tests or file writes for production.
pub trait TelemetrySink: Send + Sync + std::fmt::Debug {
    fn emit(&self, event: EvaluatorTelemetryEvent);
}

/// In-memory telemetry sink for testing and verification.
#[derive(Debug, Default)]
pub struct InMemoryTelemetrySink {
    events: Mutex<Vec<EvaluatorTelemetryEvent>>,
}

impl InMemoryTelemetrySink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<EvaluatorTelemetryEvent> {
        self.events.lock().unwrap().clone()
    }

    pub fn count_retries(&self) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, EvaluatorTelemetryEvent::Retry { .. }))
            .count()
    }

    pub fn count_circuit_changes(&self) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, EvaluatorTelemetryEvent::CircuitStateChange { .. }))
            .count()
    }

    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
    }
}

impl TelemetrySink for InMemoryTelemetrySink {
    fn emit(&self, event: EvaluatorTelemetryEvent) {
        self.events.lock().unwrap().push(event);
    }
}

/// File telemetry sink recording events to `.aida/telemetry/evaluator.jsonl`.
#[derive(Debug)]
pub struct FileTelemetrySink {
    log_path: PathBuf,
}

impl FileTelemetrySink {
    pub fn new(project_root: &Path) -> Self {
        let dir = project_root.join(".aida").join("telemetry");
        let _ = std::fs::create_dir_all(&dir);
        Self {
            log_path: dir.join("evaluator.jsonl"),
        }
    }
}

impl TelemetrySink for FileTelemetrySink {
    fn emit(&self, event: EvaluatorTelemetryEvent) {
        if let Ok(json) = serde_json::to_string(&event) {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log_path)
            {
                let _ = writeln!(file, "{json}");
            }
        }
    }
}
