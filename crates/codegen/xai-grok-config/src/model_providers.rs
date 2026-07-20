//! Stable offline model-provider directory used by `dttn config models`.
//!
//! This is intentionally metadata only. Listing a provider does not claim that
//! every provider-native wire protocol is implemented by the DTTN sampler.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProviderDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub auth_env: &'static [&'static str],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_base_url: Option<&'static str>,
    pub api_style: &'static str,
    pub local: bool,
}

pub const MODEL_PROVIDERS: &[ModelProviderDescriptor] = &[
    ModelProviderDescriptor {
        id: "deepseek",
        name: "DeepSeek",
        description: "DeepSeek API models",
        auth_env: &["DEEPSEEK_API_KEY"],
        default_base_url: Some("https://api.deepseek.com"),
        api_style: "openai-compatible",
        local: false,
    },
    ModelProviderDescriptor {
        id: "google",
        name: "Google (Gemini)",
        description: "Google Gemini models",
        auth_env: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        default_base_url: None,
        api_style: "gemini-native",
        local: false,
    },
    ModelProviderDescriptor {
        id: "lmstudio",
        name: "LM Studio (local models)",
        description: "Models served by a local LM Studio instance",
        auth_env: &["LM_API_TOKEN"],
        default_base_url: Some("http://localhost:1234/v1"),
        api_style: "openai-compatible",
        local: true,
    },
    ModelProviderDescriptor {
        id: "minimax",
        name: "MiniMax",
        description: "MiniMax API models",
        auth_env: &["MINIMAX_API_KEY"],
        default_base_url: Some("https://api.minimax.io/anthropic"),
        api_style: "anthropic-messages",
        local: false,
    },
    ModelProviderDescriptor {
        id: "moonshot",
        name: "Moonshot AI (Kimi + Kimi Coding)",
        description: "Moonshot and Kimi coding models",
        auth_env: &["MOONSHOT_API_KEY", "KIMI_API_KEY"],
        default_base_url: None,
        api_style: "openai-compatible",
        local: false,
    },
    ModelProviderDescriptor {
        id: "ollama",
        name: "Ollama (cloud + local models)",
        description: "Ollama local or remote models",
        auth_env: &["OLLAMA_API_KEY"],
        default_base_url: Some("http://localhost:11434"),
        api_style: "ollama-native",
        local: true,
    },
    ModelProviderDescriptor {
        id: "ollama-cloud",
        name: "Ollama Cloud",
        description: "Ollama-hosted cloud models",
        auth_env: &["OLLAMA_API_KEY"],
        default_base_url: Some("https://ollama.com"),
        api_style: "ollama-native",
        local: false,
    },
    ModelProviderDescriptor {
        id: "openai",
        name: "OpenAI (API + Codex)",
        description: "OpenAI API and Codex model routes",
        auth_env: &["OPENAI_API_KEY"],
        default_base_url: Some("https://api.openai.com/v1"),
        api_style: "openai",
        local: false,
    },
    ModelProviderDescriptor {
        id: "opencode",
        name: "OpenCode",
        description: "OpenCode model service",
        auth_env: &["OPENCODE_API_KEY", "OPENCODE_ZEN_API_KEY"],
        default_base_url: None,
        api_style: "openai-compatible",
        local: false,
    },
    ModelProviderDescriptor {
        id: "opencode-go",
        name: "OpenCode Go",
        description: "OpenCode Go model service",
        auth_env: &["OPENCODE_API_KEY", "OPENCODE_ZEN_API_KEY"],
        default_base_url: None,
        api_style: "openai-compatible",
        local: false,
    },
    ModelProviderDescriptor {
        id: "openrouter",
        name: "OpenRouter",
        description: "OpenRouter multi-provider model gateway",
        auth_env: &["OPENROUTER_API_KEY"],
        default_base_url: Some("https://openrouter.ai/api/v1"),
        api_style: "openai-compatible",
        local: false,
    },
    ModelProviderDescriptor {
        id: "qwen",
        name: "Qwen Cloud",
        description: "Alibaba Qwen cloud models",
        auth_env: &["QWEN_API_KEY", "MODELSTUDIO_API_KEY", "DASHSCOPE_API_KEY"],
        default_base_url: None,
        api_style: "openai-compatible",
        local: false,
    },
    ModelProviderDescriptor {
        id: "stepfun",
        name: "StepFun",
        description: "StepFun API models",
        auth_env: &["STEPFUN_API_KEY"],
        default_base_url: Some("https://api.stepfun.ai/v1"),
        api_style: "openai-compatible",
        local: false,
    },
    ModelProviderDescriptor {
        id: "xiaomi",
        name: "Xiaomi",
        description: "Xiaomi MiMo API models",
        auth_env: &["XIAOMI_API_KEY"],
        default_base_url: Some("https://api.xiaomimimo.com/v1"),
        api_style: "openai-compatible",
        local: false,
    },
    ModelProviderDescriptor {
        id: "zai",
        name: "Z.AI (GLM)",
        description: "Z.AI GLM models",
        auth_env: &["ZAI_API_KEY", "Z_AI_API_KEY"],
        default_base_url: None,
        api_style: "openai-compatible",
        local: false,
    },
    ModelProviderDescriptor {
        id: "custom",
        name: "Custom providers",
        description: "User-defined providers using DTTN-supported API backends",
        auth_env: &[],
        default_base_url: None,
        api_style: "user-defined",
        local: false,
    },
];

pub fn model_providers() -> &'static [ModelProviderDescriptor] {
    MODEL_PROVIDERS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn provider_directory_is_stable_unique_and_custom_last() {
        let ids: Vec<_> = MODEL_PROVIDERS.iter().map(|provider| provider.id).collect();
        assert_eq!(
            ids,
            [
                "deepseek",
                "google",
                "lmstudio",
                "minimax",
                "moonshot",
                "ollama",
                "ollama-cloud",
                "openai",
                "opencode",
                "opencode-go",
                "openrouter",
                "qwen",
                "stepfun",
                "xiaomi",
                "zai",
                "custom",
            ]
        );
        assert_eq!(ids.iter().copied().collect::<HashSet<_>>().len(), ids.len());
        assert_eq!(MODEL_PROVIDERS.last().unwrap().name, "Custom providers");
    }
}
