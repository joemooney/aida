//! EvaluatorEngine trait, TypeSafe Jev System One implementation, and MockEvaluator.
//!
//! Provides fast, calibrated, typed evaluation primitives (noul, choice, score)
//! for automated quality gating, graded review, and store-wide contradiction sweep.
//!
//! Complies with PRIN-5 (fail-closed), PRIN-6 (currency & provenance),
//! PRIN-7 (dual predicates), and PRIN-8 (calibrated heuristic at Rung 3.5).
//
// trace:ADR-55 | ai:antigravity

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Evaluated noul (yes/no probability) response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoulResponse {
    /// Calibrated probability in [0.0, 1.0].
    pub noul: f64,
    /// Evaluator-reported calibration confidence in [0.0, 1.0].
    pub confidence: f64,
    /// Explicit heuristic marker (PRIN-8).
    pub heuristic: bool,
    /// Model identifier that rendered the judgment.
    pub model: String,
    /// SHA-256 of the exact question payload sent to the evaluator.
    pub payload_hash: String,
}

/// Evaluated categorical choice response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChoiceResponse {
    /// Selected choice key.
    pub choice: String,
    /// Calibrated confidence in [0.0, 1.0].
    pub confidence: f64,
    /// Probability distribution across all offered options.
    pub probabilities: HashMap<String, f64>,
    /// Explicit heuristic marker (PRIN-8).
    pub heuristic: bool,
    /// Model identifier that rendered the judgment.
    pub model: String,
    pub payload_hash: String,
}

/// Evaluated scalar score response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScoreResponse {
    /// Scalar score in [0.0, 1.0].
    pub score: f64,
    pub confidence: f64,
    /// Explicit heuristic marker (PRIN-8).
    pub heuristic: bool,
    /// Model identifier that rendered the judgment.
    pub model: String,
    pub payload_hash: String,
}

fn payload_hash(payload: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec(payload).unwrap_or_default();
    format!("{:x}", Sha256::digest(bytes))
}

/// Typed evaluator failure modes for fail-closed handling (PRIN-5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluatorError {
    MissingApiKey(String),
    Network(String),
    ApiError { status: u16, message: String },
    ParseError(String),
    Timeout(String),
    Other(String),
}

impl std::fmt::Display for EvaluatorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingApiKey(msg) => write!(f, "Missing API key: {msg}"),
            Self::Network(msg) => write!(f, "Network error: {msg}"),
            Self::ApiError { status, message } => write!(f, "API error ({status}): {message}"),
            Self::ParseError(msg) => write!(f, "Parse error: {msg}"),
            Self::Timeout(msg) => write!(f, "Timeout: {msg}"),
            Self::Other(msg) => write!(f, "Evaluator error: {msg}"),
        }
    }
}

impl std::error::Error for EvaluatorError {}

pub type EvaluatorFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, EvaluatorError>> + Send + 'a>>;

/// Abstract evaluation engine (ADR-55).
pub trait EvaluatorEngine: Send + Sync {
    /// Evaluate a yes/no statement into a calibrated probability.
    fn evaluate_noul<'a>(
        &'a self,
        context: &'a str,
        instruction: &'a str,
    ) -> EvaluatorFuture<'a, NoulResponse>;

    /// Evaluate a categorical choice across discrete options.
    fn evaluate_choice<'a>(
        &'a self,
        context: &'a str,
        instruction: &'a str,
        options: &'a HashMap<String, String>,
    ) -> EvaluatorFuture<'a, ChoiceResponse>;

    /// Evaluate an ordinal quality score across ordered levels.
    fn evaluate_score<'a>(
        &'a self,
        context: &'a str,
        instruction: &'a str,
        levels: &'a [String],
    ) -> EvaluatorFuture<'a, ScoreResponse>;

    /// Synchronous convenience helper for `evaluate_noul`.
    fn evaluate_noul_sync(
        &self,
        context: &str,
        instruction: &str,
    ) -> Result<NoulResponse, EvaluatorError> {
        block_on(self.evaluate_noul(context, instruction))
    }

    /// Synchronous convenience helper for `evaluate_choice`.
    fn evaluate_choice_sync(
        &self,
        context: &str,
        instruction: &str,
        options: &HashMap<String, String>,
    ) -> Result<ChoiceResponse, EvaluatorError> {
        block_on(self.evaluate_choice(context, instruction, options))
    }

    /// Synchronous convenience helper for `evaluate_score`.
    fn evaluate_score_sync(
        &self,
        context: &str,
        instruction: &str,
        levels: &[String],
    ) -> Result<ScoreResponse, EvaluatorError> {
        block_on(self.evaluate_score(context, instruction, levels))
    }
}

/// Helper to execute an async future synchronously from sync contexts.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(future)),
        Err(_) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to build tokio runtime");
            rt.block_on(future)
        }
    }
}

/// Concrete client connecting to TypeSafe AI's System One API (Jev).
// trace:ADR-55 | ai:antigravity
pub struct JevEvaluator {
    client: reqwest::Client,
    request_timeout: std::time::Duration,
    api_key: String,
    endpoint: String,
    model: String,
}

impl JevEvaluator {
    pub const DEFAULT_ENDPOINT: &'static str = "https://api.typesafe.ai/v1/systemone";
    pub const DEFAULT_MODEL: &'static str = "jev-1.13.0";
    pub const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

    /// Create a new Jev evaluator with given API key and default options.
    pub fn new(api_key: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Self::DEFAULT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let endpoint = std::env::var("AIDA_JEV_ENDPOINT")
            .unwrap_or_else(|_| Self::DEFAULT_ENDPOINT.to_string());
        let model =
            std::env::var("AIDA_JEV_MODEL").unwrap_or_else(|_| Self::DEFAULT_MODEL.to_string());

        Self {
            client,
            request_timeout: Self::DEFAULT_TIMEOUT,
            api_key: api_key.into(),
            endpoint,
            model,
        }
    }

    /// Attempt to construct a JevEvaluator by inspecting process environment and `~/.env`.
    pub fn from_env() -> Result<Self, EvaluatorError> {
        if let Ok(key) = std::env::var("AIDA_JEV_API_KEY") {
            if !key.trim().is_empty() {
                return Ok(Self::new(key.trim()));
            }
        }
        if let Ok(key) = std::env::var("TYPESAFE_API_KEY") {
            if !key.trim().is_empty() {
                return Ok(Self::new(key.trim()));
            }
        }

        // Try reading ~/.env
        if let Some(home) = dirs::home_dir() {
            let env_path = home.join(".env");
            if env_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&env_path) {
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if trimmed.starts_with('#') || trimmed.is_empty() {
                            continue;
                        }
                        if let Some((k, v)) = trimmed.split_once('=') {
                            let key_name = k.trim();
                            if key_name == "AIDA_JEV_API_KEY" || key_name == "TYPESAFE_API_KEY" {
                                let clean_val =
                                    v.trim().trim_matches('"').trim_matches('\'').trim();
                                if !clean_val.is_empty() {
                                    return Ok(Self::new(clean_val));
                                }
                            }
                        }
                    }
                }
            }
        }

        Err(EvaluatorError::MissingApiKey(
            "Neither AIDA_JEV_API_KEY nor TYPESAFE_API_KEY was found in environment or ~/.env"
                .to_string(),
        ))
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Minimal evaluator shim for advisory callers (`aida explain`): replace the
    /// default 15s request timeout with a caller-chosen deadline so an advisory
    /// audit can never stall the command. Stands in for the unmerged resilience
    /// adapter's `into_resilient` on the feature branch.
    // trace:TASK-1470 | ai:claude
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        match reqwest::Client::builder().timeout(timeout).build() {
            Ok(client) => {
                self.client = client;
                self.request_timeout = timeout;
            }
            Err(err) => eprintln!(
                "Note: could not apply a {}s evaluator timeout ({err}); keeping the {}s default.",
                timeout.as_secs(),
                self.request_timeout.as_secs()
            ),
        }
        self
    }

    /// The request timeout the HTTP client was actually built with.
    // trace:TASK-1470 | ai:claude
    pub fn request_timeout(&self) -> std::time::Duration {
        self.request_timeout
    }
}

impl EvaluatorEngine for JevEvaluator {
    fn evaluate_noul<'a>(
        &'a self,
        context: &'a str,
        instruction: &'a str,
    ) -> EvaluatorFuture<'a, NoulResponse> {
        Box::pin(async move {
            let payload = serde_json::json!({
                "model": self.model,
                "state": context,
                "questions": {
                    "q": {
                        "type": "noul",
                        "instructions": instruction
                    }
                }
            });
            let question_payload_hash = payload_hash(&payload);

            let resp = self
                .client
                .post(&self.endpoint)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&payload)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        EvaluatorError::Timeout(e.to_string())
                    } else {
                        EvaluatorError::Network(e.to_string())
                    }
                })?;

            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(EvaluatorError::ApiError {
                    status: status.as_u16(),
                    message: body,
                });
            }

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| EvaluatorError::ParseError(e.to_string()))?;

            let answer = body
                .get("answers")
                .and_then(|a| a.get("q"))
                .ok_or_else(|| {
                    EvaluatorError::ParseError("Missing answer key in response".to_string())
                })?;

            let noul = answer.get("noul").and_then(|n| n.as_f64()).ok_or_else(|| {
                EvaluatorError::ParseError("Invalid or missing noul float".to_string())
            })?;
            let confidence = answer
                .get("confidence")
                .and_then(|c| c.as_f64())
                .ok_or_else(|| {
                    EvaluatorError::ParseError("Missing confidence field".to_string())
                })?;

            let model = body
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or(&self.model)
                .to_string();

            Ok(NoulResponse {
                noul,
                confidence,
                heuristic: true,
                model,
                payload_hash: question_payload_hash,
            })
        })
    }

    fn evaluate_choice<'a>(
        &'a self,
        context: &'a str,
        instruction: &'a str,
        options: &'a HashMap<String, String>,
    ) -> EvaluatorFuture<'a, ChoiceResponse> {
        Box::pin(async move {
            let payload = serde_json::json!({
                "model": self.model,
                "state": context,
                "questions": {
                    "q": {
                        "type": "choice",
                        "instructions": instruction,
                        "criteria": options
                    }
                }
            });
            let question_payload_hash = payload_hash(&payload);

            let resp = self
                .client
                .post(&self.endpoint)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&payload)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        EvaluatorError::Timeout(e.to_string())
                    } else {
                        EvaluatorError::Network(e.to_string())
                    }
                })?;

            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(EvaluatorError::ApiError {
                    status: status.as_u16(),
                    message: body,
                });
            }

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| EvaluatorError::ParseError(e.to_string()))?;

            let answer = body
                .get("answers")
                .and_then(|a| a.get("q"))
                .ok_or_else(|| {
                    EvaluatorError::ParseError("Missing answer key in response".to_string())
                })?;

            let choice = answer
                .get("choice")
                .and_then(|c| c.as_str())
                .ok_or_else(|| EvaluatorError::ParseError("Missing choice field".to_string()))?
                .to_string();

            let confidence = answer
                .get("confidence")
                .and_then(|c| c.as_f64())
                .unwrap_or(0.0);

            let mut probabilities = HashMap::new();
            if let Some(probs) = answer.get("probabilities").and_then(|p| p.as_object()) {
                for (k, v) in probs {
                    if let Some(p) = v.as_f64() {
                        probabilities.insert(k.clone(), p);
                    }
                }
            }

            let model = body
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or(&self.model)
                .to_string();

            Ok(ChoiceResponse {
                choice,
                confidence,
                probabilities,
                heuristic: true,
                model,
                payload_hash: question_payload_hash,
            })
        })
    }

    fn evaluate_score<'a>(
        &'a self,
        context: &'a str,
        instruction: &'a str,
        levels: &'a [String],
    ) -> EvaluatorFuture<'a, ScoreResponse> {
        Box::pin(async move {
            let payload = serde_json::json!({
                "model": self.model,
                "state": context,
                "questions": {
                    "q": {
                        "type": "score",
                        "instructions": instruction,
                        "levels": levels
                    }
                }
            });
            let question_payload_hash = payload_hash(&payload);

            let resp = self
                .client
                .post(&self.endpoint)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&payload)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        EvaluatorError::Timeout(e.to_string())
                    } else {
                        EvaluatorError::Network(e.to_string())
                    }
                })?;

            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(EvaluatorError::ApiError {
                    status: status.as_u16(),
                    message: body,
                });
            }

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| EvaluatorError::ParseError(e.to_string()))?;

            let answer = body
                .get("answers")
                .and_then(|a| a.get("q"))
                .ok_or_else(|| {
                    EvaluatorError::ParseError("Missing answer key in response".to_string())
                })?;

            let score = answer
                .get("score")
                .and_then(|s| s.as_f64())
                .ok_or_else(|| EvaluatorError::ParseError("Missing score field".to_string()))?;
            let confidence = answer
                .get("confidence")
                .and_then(|c| c.as_f64())
                .ok_or_else(|| {
                    EvaluatorError::ParseError("Missing confidence field".to_string())
                })?;

            let model = body
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or(&self.model)
                .to_string();

            Ok(ScoreResponse {
                score,
                confidence,
                heuristic: true,
                model,
                payload_hash: question_payload_hash,
            })
        })
    }
}

/// Offline-sovereign evaluator for an OpenAI-compatible local Ollama/vLLM endpoint.
/// The endpoint is never inferred from a remote URL: callers must opt in with
/// `AIDA_LOCAL_LLM_ENDPOINT` (defaulting to Ollama on loopback).
// trace:ADR-55 | ai:codex
pub struct LocalLlmEvaluator {
    client: reqwest::Client,
    endpoint: String,
    model: String,
}

impl LocalLlmEvaluator {
    pub fn from_env() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            endpoint: std::env::var("AIDA_LOCAL_LLM_ENDPOINT")
                .unwrap_or_else(|_| "http://127.0.0.1:11434/v1/chat/completions".into()),
            model: std::env::var("AIDA_LOCAL_LLM_MODEL").unwrap_or_else(|_| "qwen2.5:7b".into()),
        }
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    async fn evaluate_json(
        &self,
        context: &str,
        instruction: &str,
        schema: serde_json::Value,
    ) -> Result<(serde_json::Value, String), EvaluatorError> {
        let payload = serde_json::json!({
            "model": self.model,
            "messages": [{"role":"user", "content": format!("{}\n\n{}", context, instruction)}],
            "response_format": {"type":"json_schema", "json_schema":{"name":"aida_evaluation", "strict":true, "schema":schema}},
            "temperature": 0
        });
        let hash = payload_hash(&payload);
        let response = self
            .client
            .post(&self.endpoint)
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    EvaluatorError::Timeout(e.to_string())
                } else {
                    EvaluatorError::Network(e.to_string())
                }
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(EvaluatorError::ApiError {
                status: status.as_u16(),
                message: response.text().await.unwrap_or_default(),
            });
        }
        let envelope: serde_json::Value = response
            .json()
            .await
            .map_err(|e| EvaluatorError::ParseError(e.to_string()))?;
        let content = envelope
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                EvaluatorError::ParseError(
                    "local LLM response omitted choices[0].message.content".into(),
                )
            })?;
        let value =
            serde_json::from_str(content).map_err(|e| EvaluatorError::ParseError(e.to_string()))?;
        Ok((value, hash))
    }
}

impl EvaluatorEngine for LocalLlmEvaluator {
    fn evaluate_noul<'a>(
        &'a self,
        context: &'a str,
        instruction: &'a str,
    ) -> EvaluatorFuture<'a, NoulResponse> {
        Box::pin(async move {
            let schema = serde_json::json!({"type":"object","properties":{"noul":{"type":"number"},"confidence":{"type":"number"}},"required":["noul","confidence"],"additionalProperties":false});
            let (v, hash) = self.evaluate_json(context, instruction, schema).await?;
            Ok(NoulResponse {
                noul: number(&v, "noul")?,
                confidence: number(&v, "confidence")?,
                heuristic: true,
                model: self.model.clone(),
                payload_hash: hash,
            })
        })
    }

    fn evaluate_choice<'a>(
        &'a self,
        context: &'a str,
        instruction: &'a str,
        options: &'a HashMap<String, String>,
    ) -> EvaluatorFuture<'a, ChoiceResponse> {
        Box::pin(async move {
            let keys: Vec<&str> = options.keys().map(String::as_str).collect();
            let schema = serde_json::json!({"type":"object","properties":{"choice":{"type":"string","enum":keys},"confidence":{"type":"number"},"probabilities":{"type":"object","additionalProperties":{"type":"number"}}},"required":["choice","confidence","probabilities"],"additionalProperties":false});
            let (v, hash) = self.evaluate_json(context, instruction, schema).await?;
            let probabilities =
                serde_json::from_value(v.get("probabilities").cloned().unwrap_or_default())
                    .map_err(|e| EvaluatorError::ParseError(e.to_string()))?;
            Ok(ChoiceResponse {
                choice: v
                    .get("choice")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| EvaluatorError::ParseError("missing choice".into()))?
                    .into(),
                confidence: number(&v, "confidence")?,
                probabilities,
                heuristic: true,
                model: self.model.clone(),
                payload_hash: hash,
            })
        })
    }

    fn evaluate_score<'a>(
        &'a self,
        context: &'a str,
        instruction: &'a str,
        _levels: &'a [String],
    ) -> EvaluatorFuture<'a, ScoreResponse> {
        Box::pin(async move {
            let schema = serde_json::json!({"type":"object","properties":{"score":{"type":"number"},"confidence":{"type":"number"}},"required":["score","confidence"],"additionalProperties":false});
            let (v, hash) = self.evaluate_json(context, instruction, schema).await?;
            Ok(ScoreResponse {
                score: number(&v, "score")?,
                confidence: number(&v, "confidence")?,
                heuristic: true,
                model: self.model.clone(),
                payload_hash: hash,
            })
        })
    }
}

fn number(value: &serde_json::Value, key: &str) -> Result<f64, EvaluatorError> {
    value
        .get(key)
        .and_then(|v| v.as_f64())
        .filter(|v| (0.0..=1.0).contains(v))
        .ok_or_else(|| EvaluatorError::ParseError(format!("missing or out-of-range {key}")))
}

/// Deterministic mock evaluator for offline tests and fixture verification.
// trace:ADR-55 | ai:antigravity
#[derive(Debug, Clone, Default)]
pub struct MockEvaluator {
    pub noul_queue: Arc<Mutex<Vec<Result<NoulResponse, EvaluatorError>>>>,
    pub choice_queue: Arc<Mutex<Vec<Result<ChoiceResponse, EvaluatorError>>>>,
    pub score_queue: Arc<Mutex<Vec<Result<ScoreResponse, EvaluatorError>>>>,
}

impl MockEvaluator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_noul(self, noul: f64) -> Self {
        self.with_noul_confidence(noul, 0.95)
    }

    pub fn with_noul_confidence(self, noul: f64, confidence: f64) -> Self {
        self.noul_queue.lock().unwrap().push(Ok(NoulResponse {
            noul,
            confidence,
            heuristic: true,
            model: "mock-jev".to_string(),
            payload_hash: "mock-payload".to_string(),
        }));
        self
    }

    pub fn with_choice(
        self,
        choice: &str,
        confidence: f64,
        probabilities: HashMap<String, f64>,
    ) -> Self {
        self.choice_queue.lock().unwrap().push(Ok(ChoiceResponse {
            choice: choice.to_string(),
            confidence,
            probabilities,
            heuristic: true,
            model: "mock-jev".to_string(),
            payload_hash: "mock-payload".to_string(),
        }));
        self
    }

    pub fn with_score(self, score: f64) -> Self {
        self.score_queue.lock().unwrap().push(Ok(ScoreResponse {
            score,
            confidence: 0.95,
            heuristic: true,
            model: "mock-jev".to_string(),
            payload_hash: "mock-payload".to_string(),
        }));
        self
    }

    pub fn with_noul_error(self, err: EvaluatorError) -> Self {
        self.noul_queue.lock().unwrap().push(Err(err));
        self
    }

    pub fn with_choice_error(self, err: EvaluatorError) -> Self {
        self.choice_queue.lock().unwrap().push(Err(err));
        self
    }
}

impl EvaluatorEngine for MockEvaluator {
    fn evaluate_noul<'a>(
        &'a self,
        _context: &'a str,
        _instruction: &'a str,
    ) -> EvaluatorFuture<'a, NoulResponse> {
        Box::pin(async move {
            let mut queue = self.noul_queue.lock().unwrap();
            if !queue.is_empty() {
                queue.remove(0)
            } else {
                Ok(NoulResponse {
                    noul: 1.0,
                    confidence: 1.0,
                    heuristic: true,
                    model: "mock-jev".to_string(),
                    payload_hash: "mock-payload".to_string(),
                })
            }
        })
    }

    fn evaluate_choice<'a>(
        &'a self,
        _context: &'a str,
        _instruction: &'a str,
        options: &'a HashMap<String, String>,
    ) -> EvaluatorFuture<'a, ChoiceResponse> {
        Box::pin(async move {
            let mut queue = self.choice_queue.lock().unwrap();
            if !queue.is_empty() {
                queue.remove(0)
            } else {
                let first_opt = options
                    .keys()
                    .next()
                    .cloned()
                    .unwrap_or_else(|| "compatible".to_string());
                let mut probs = HashMap::new();
                probs.insert(first_opt.clone(), 1.0);
                Ok(ChoiceResponse {
                    choice: first_opt,
                    confidence: 1.0,
                    probabilities: probs,
                    heuristic: true,
                    model: "mock-jev".to_string(),
                    payload_hash: "mock-payload".to_string(),
                })
            }
        })
    }

    fn evaluate_score<'a>(
        &'a self,
        _context: &'a str,
        _instruction: &'a str,
        _levels: &'a [String],
    ) -> EvaluatorFuture<'a, ScoreResponse> {
        Box::pin(async move {
            let mut queue = self.score_queue.lock().unwrap();
            if !queue.is_empty() {
                queue.remove(0)
            } else {
                Ok(ScoreResponse {
                    score: 1.0,
                    confidence: 1.0,
                    heuristic: true,
                    model: "mock-jev".to_string(),
                    payload_hash: "mock-payload".to_string(),
                })
            }
        })
    }
}
