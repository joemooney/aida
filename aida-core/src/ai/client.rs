//! AI Client Module
//!
//! Handles communication with Claude via CLI or direct API.

use crate::ai::prompts;
use crate::ai::responses::{
    self, DuplicatesResponse, EvaluationResponse, GenerateChildrenResponse,
    ImproveDescriptionResponse, SuggestRelationshipsResponse,
};
use crate::models::{Requirement, RequirementsStore};
use std::path::PathBuf;
use std::process::Command;
use thiserror::Error;
use uuid::Uuid;

/// Errors that can occur during AI operations
#[derive(Error, Debug)]
pub enum AiError {
    #[error("Claude CLI not found at {0}")]
    CliNotFound(PathBuf),

    #[error("Claude CLI execution failed: {0}")]
    CliExecFailed(String),

    #[error("API key missing")]
    ApiKeyMissing,

    #[error("API request failed: {0}")]
    ApiRequestFailed(String),

    #[error("Invalid response from AI: {0}")]
    InvalidResponse(String),

    #[error("Rate limited - please wait before retrying")]
    RateLimited,

    #[error("Context too large for AI model")]
    ContextTooLarge,

    #[error("Requirement not found: {0}")]
    RequirementNotFound(Uuid),

    #[error("AI integration not available")]
    NotAvailable,
}

/// AI operation mode
#[derive(Debug, Clone, Default)]
pub enum AiMode {
    /// Use Claude CLI with --print flag
    ClaudeCli { path: PathBuf },
    /// Direct API integration (future)
    DirectApi { api_key: String },
    /// AI features disabled
    #[default]
    Disabled,
}

/// AI Client for interacting with Claude
#[derive(Debug, Clone)]
pub struct AiClient {
    mode: AiMode,
}

impl Default for AiClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AiClient {
    /// Create a new AI client with auto-detected mode
    pub fn new() -> Self {
        let mode = Self::detect_mode();
        Self { mode }
    }

    /// Create a client with a specific mode
    pub fn with_mode(mode: AiMode) -> Self {
        Self { mode }
    }

    /// Detect the best available AI mode
    fn detect_mode() -> AiMode {
        // Try to find claude CLI
        if let Some(path) = Self::find_claude_cli() {
            return AiMode::ClaudeCli { path };
        }

        // Could check for API key in environment
        if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
            if !api_key.is_empty() {
                return AiMode::DirectApi { api_key };
            }
        }

        AiMode::Disabled
    }

    /// Find the claude CLI executable
    fn find_claude_cli() -> Option<PathBuf> {
        // Common locations to check
        let candidates = [
            // In PATH
            "claude",
            // npm global install locations
            "/usr/local/bin/claude",
            "/usr/bin/claude",
        ];

        // First check if 'claude' is in PATH
        if let Ok(output) = Command::new("which").arg("claude").output() {
            if output.status.success() {
                let path_str = String::from_utf8_lossy(&output.stdout);
                let path = PathBuf::from(path_str.trim());
                if path.exists() {
                    return Some(path);
                }
            }
        }

        // Check common locations
        for candidate in candidates {
            let path = PathBuf::from(candidate);
            if path.exists() {
                return Some(path);
            }
        }

        // Check home directory npm global
        if let Ok(home) = std::env::var("HOME") {
            let npm_global = PathBuf::from(home).join(".npm-global/bin/claude");
            if npm_global.exists() {
                return Some(npm_global);
            }
        }

        None
    }

    /// Check if AI features are available
    pub fn is_available(&self) -> bool {
        match &self.mode {
            AiMode::ClaudeCli { path } => path.exists(),
            AiMode::DirectApi { api_key } => !api_key.is_empty(),
            AiMode::Disabled => false,
        }
    }

    /// Get the current mode
    pub fn mode(&self) -> &AiMode {
        &self.mode
    }

    /// Get a description of the current mode
    pub fn mode_description(&self) -> String {
        match &self.mode {
            AiMode::ClaudeCli { path } => format!("Claude CLI ({})", path.display()),
            AiMode::DirectApi { .. } => "Direct API".to_string(),
            AiMode::Disabled => "Disabled".to_string(),
        }
    }

    /// Evaluate a requirement's quality
    pub fn evaluate_requirement(
        &self,
        req: &Requirement,
        store: &RequirementsStore,
    ) -> Result<EvaluationResponse, AiError> {
        let prompt = prompts::build_evaluation_prompt(req, store);
        let response = self.send_request(&prompt)?;
        responses::parse_evaluation_response(&response)
    }

    /// Find potential duplicate requirements
    pub fn find_duplicates(
        &self,
        req: &Requirement,
        store: &RequirementsStore,
    ) -> Result<DuplicatesResponse, AiError> {
        let prompt = prompts::build_duplicates_prompt(req, store);
        let response = self.send_request(&prompt)?;
        responses::parse_duplicates_response(&response)
    }

    /// Suggest relationships for a requirement
    pub fn suggest_relationships(
        &self,
        req: &Requirement,
        store: &RequirementsStore,
    ) -> Result<SuggestRelationshipsResponse, AiError> {
        let prompt = prompts::build_relationships_prompt(req, store);
        let response = self.send_request(&prompt)?;
        responses::parse_relationships_response(&response)
    }

    /// Improve a requirement's description
    pub fn improve_description(
        &self,
        req: &Requirement,
        store: &RequirementsStore,
    ) -> Result<ImproveDescriptionResponse, AiError> {
        let prompt = prompts::build_improve_prompt(req, store);
        let response = self.send_request(&prompt)?;
        responses::parse_improve_response(&response)
    }

    /// Generate child requirements
    pub fn generate_children(
        &self,
        req: &Requirement,
        store: &RequirementsStore,
    ) -> Result<GenerateChildrenResponse, AiError> {
        let prompt = prompts::build_generate_children_prompt(req, store);
        let response = self.send_request(&prompt)?;
        responses::parse_generate_children_response(&response)
    }

    /// Send a request to the AI
    fn send_request(&self, prompt: &str) -> Result<String, AiError> {
        match &self.mode {
            AiMode::ClaudeCli { path } => self.send_cli_request(path, prompt),
            AiMode::DirectApi { api_key: _ } => {
                // Future: implement direct API
                Err(AiError::NotAvailable)
            }
            AiMode::Disabled => Err(AiError::NotAvailable),
        }
    }

    /// Send request via Claude CLI
    fn send_cli_request(&self, cli_path: &PathBuf, prompt: &str) -> Result<String, AiError> {
        // Use --print flag for non-interactive output
        // Use -p flag to pass the prompt
        let output = Command::new(cli_path)
            .arg("--print")
            .arg("-p")
            .arg(prompt)
            .output()
            .map_err(|e| AiError::CliExecFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AiError::CliExecFailed(format!(
                "Exit code: {:?}, stderr: {}",
                output.status.code(),
                stderr
            )));
        }

        let response = String::from_utf8_lossy(&output.stdout).to_string();

        if response.is_empty() {
            return Err(AiError::InvalidResponse(
                "Empty response from CLI".to_string(),
            ));
        }

        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_detection() {
        let client = AiClient::new();
        // Just ensure it doesn't panic
        let _ = client.is_available();
        let _ = client.mode_description();
    }

    #[test]
    fn test_disabled_mode() {
        let client = AiClient::with_mode(AiMode::Disabled);
        assert!(!client.is_available());
        assert_eq!(client.mode_description(), "Disabled");
    }
}
