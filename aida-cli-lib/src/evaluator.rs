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
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Evaluated noul (yes/no probability) response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoulResponse {
    /// Calibrated probability in [0.0, 1.0].
    pub noul: f64,
    /// Explicit heuristic marker (PRIN-8).
    pub heuristic: bool,
    /// Model identifier that rendered the judgment.
    pub model: String,
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
}

/// Evaluated scalar score response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScoreResponse {
    /// Scalar score in [0.0, 1.0].
    pub score: f64,
    /// Explicit heuristic marker (PRIN-8).
    pub heuristic: bool,
    /// Model identifier that rendered the judgment.
    pub model: String,
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
    api_key: String,
    endpoint: String,
    model: String,
}

impl JevEvaluator {
    pub const DEFAULT_ENDPOINT: &'static str = "https://api.typesafe.ai/v1/systemone";
    pub const DEFAULT_MODEL: &'static str = "jev-1.13.0";

    /// Create a new Jev evaluator with given API key and default options.
    pub fn new(api_key: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let endpoint = std::env::var("AIDA_JEV_ENDPOINT")
            .unwrap_or_else(|_| Self::DEFAULT_ENDPOINT.to_string());
        let model =
            std::env::var("AIDA_JEV_MODEL").unwrap_or_else(|_| Self::DEFAULT_MODEL.to_string());

        Self {
            client,
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

            let model = body
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or(&self.model)
                .to_string();

            Ok(NoulResponse {
                noul,
                heuristic: true,
                model,
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

            let model = body
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or(&self.model)
                .to_string();

            Ok(ScoreResponse {
                score,
                heuristic: true,
                model,
            })
        })
    }
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
        self.noul_queue.lock().unwrap().push(Ok(NoulResponse {
            noul,
            heuristic: true,
            model: "mock-jev".to_string(),
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
        }));
        self
    }

    pub fn with_score(self, score: f64) -> Self {
        self.score_queue.lock().unwrap().push(Ok(ScoreResponse {
            score,
            heuristic: true,
            model: "mock-jev".to_string(),
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
                    heuristic: true,
                    model: "mock-jev".to_string(),
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
                    heuristic: true,
                    model: "mock-jev".to_string(),
                })
            }
        })
    }
}
