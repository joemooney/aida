//! AI Integration Module for AIDA
//!
//! This module provides AI-powered analysis and suggestions for requirements
//! management using Claude Code CLI integration.

pub mod client;
pub mod evaluator;
pub mod prompts;
pub mod responses;

pub use client::{AiClient, AiError, AiMode};
pub use evaluator::{BackgroundEvaluator, EvaluationResult, EvaluatorConfig, EvaluatorStatus};
pub use responses::{
    DraftSpecResponse, DuplicateResult, EvaluationResponse, GeneratedChild, IssueReport,
    RelationshipSuggestion, StoredAiEvaluation, SuggestedImprovement,
};
