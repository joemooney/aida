// Shared LLM provider selection and defaults for server AI endpoints.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmProvider {
    Anthropic,
    OpenAi,
}

impl LlmProvider {
    pub fn from_env() -> Self {
        match std::env::var("AIDA_AI_PROVIDER")
            .unwrap_or_else(|_| "anthropic".to_string())
            .to_lowercase()
            .as_str()
        {
            "openai" | "codex" | "gpt" => LlmProvider::OpenAi,
            _ => LlmProvider::Anthropic,
        }
    }

    pub fn api_key_name(&self) -> &'static str {
        match self {
            LlmProvider::Anthropic => "ANTHROPIC_API_KEY",
            LlmProvider::OpenAi => "OPENAI_API_KEY",
        }
    }

    pub fn default_chat_model(&self) -> &'static str {
        match self {
            LlmProvider::Anthropic => "claude-sonnet-4-6",
            LlmProvider::OpenAi => "codex-mini-latest",
        }
    }

    pub fn default_eval_model(&self) -> &'static str {
        match self {
            LlmProvider::Anthropic => "claude-sonnet-4-20250514",
            LlmProvider::OpenAi => "gpt-5-mini",
        }
    }

    pub fn base_url(&self) -> String {
        match self {
            LlmProvider::Anthropic => std::env::var("AIDA_ANTHROPIC_BASE_URL")
                .unwrap_or_else(|_| "https://api.anthropic.com".to_string()),
            LlmProvider::OpenAi => std::env::var("AIDA_OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".to_string()),
        }
    }

    pub fn resolve_model(&self, env_name: &str, default_model: &str) -> String {
        std::env::var(env_name).unwrap_or_else(|_| default_model.to_string())
    }
}
