#!/usr/bin/env python3
"""Apply the deterministic DTTN Phase 3 provider-compatibility migration.

The migration separates API protocol shape from provider-private extensions.
Every semantic replacement is strict; struct-literal updates are syntax-aware
and only touch SamplingConfig/SamplerConfig literals containing model/base_url.
"""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    (ROOT / path).write_text(text, encoding="utf-8")


def replace_exact(path: str, old: str, new: str, expected: int = 1) -> None:
    text = read(path)
    count = text.count(old)
    if count == 0 and new in text:
        return
    if count != expected:
        raise RuntimeError(
            f"{path}: expected {expected} occurrence(s), found {count}: {old!r}"
        )
    write(path, text.replace(old, new, expected))


def matching_brace(text: str, open_index: int) -> int:
    depth = 0
    i = open_index
    state = "code"
    while i < len(text):
        ch = text[i]
        nxt = text[i + 1] if i + 1 < len(text) else ""
        if state == "code":
            if ch == '"':
                state = "string"
            elif ch == "'":
                state = "char"
            elif ch == "/" and nxt == "/":
                state = "line_comment"
                i += 1
            elif ch == "/" and nxt == "*":
                state = "block_comment"
                i += 1
            elif ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
                if depth == 0:
                    return i
        elif state == "string":
            if ch == "\\":
                i += 1
            elif ch == '"':
                state = "code"
        elif state == "char":
            if ch == "\\":
                i += 1
            elif ch == "'":
                state = "code"
        elif state == "line_comment":
            if ch == "\n":
                state = "code"
        elif state == "block_comment":
            if ch == "*" and nxt == "/":
                state = "code"
                i += 1
        i += 1
    raise RuntimeError("unbalanced Rust literal")


def add_provider_field_to_literals(path: Path, type_name: str) -> bool:
    text = path.read_text(encoding="utf-8")
    pattern = re.compile(rf"(?<![A-Za-z0-9_]){re.escape(type_name)}\s*\{{")
    edits: list[tuple[int, str]] = []
    for match in pattern.finditer(text):
        line_start = text.rfind("\n", 0, match.start()) + 1
        prefix = text[line_start : match.start()]
        if re.search(r"\b(struct|enum)\s+$", prefix):
            continue
        open_index = text.find("{", match.start(), match.end())
        close_index = matching_brace(text, open_index)
        block = text[open_index + 1 : close_index]
        if "provider_extensions:" in block:
            continue
        if not all(marker in block for marker in ("base_url:", "model:", "api_backend:")):
            continue
        api_match = re.search(
            r"(?m)^(?P<indent>[ \t]*)api_backend\s*:\s*[^\n]+,\s*$", block
        )
        if not api_match:
            raise RuntimeError(f"{path}: could not locate api_backend field in {type_name} literal")
        insert_at = open_index + 1 + api_match.end()
        indent = api_match.group("indent")
        edits.append((insert_at, f"\n{indent}provider_extensions: Default::default(),"))
    if not edits:
        return False
    for index, value in reversed(edits):
        text = text[:index] + value + text[index:]
    path.write_text(text, encoding="utf-8")
    return True


def apply_sampling_types() -> None:
    path = "crates/codegen/xai-grok-sampling-types/src/types.rs"
    replace_exact(
        path,
        '''impl ApiBackend {
    /// Whether the backend enforces a response JSON schema natively alongside
    /// tool calls. The Messages API does not (a schema there blocks tool use),
    /// so structured output there goes through the StructuredOutput tool.
    pub fn supports_native_schema(&self) -> bool {
        matches!(self, Self::ChatCompletions | Self::Responses)
    }
}

/// Sampling client configuration (API key excluded — that stays in the client).''',
        '''impl ApiBackend {
    /// Whether the backend enforces a response JSON schema natively alongside
    /// tool calls. The Messages API does not (a schema there blocks tool use),
    /// so structured output there goes through the StructuredOutput tool.
    pub fn supports_native_schema(&self) -> bool {
        matches!(self, Self::ChatCompletions | Self::Responses)
    }
}

/// Provider-private wire extensions layered on top of the standard API shape.
///
/// `ApiBackend` chooses the request/response protocol. This enum independently
/// controls non-standard headers and body fields. `Auto` preserves compatibility
/// for official xAI/Grok hosts while treating every other endpoint as standard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderExtensions {
    #[default]
    Auto,
    Standard,
    Xai,
}

impl ProviderExtensions {
    pub const fn uses_xai(self) -> bool {
        matches!(self, Self::Xai)
    }
}

/// Sampling client configuration (API key excluded — that stays in the client).''',
    )
    replace_exact(
        path,
        '''    #[serde(default)]
    pub api_backend: ApiBackend,
    /// Extra headers to send with requests (e.g., for BYOK scenarios).''',
        '''    #[serde(default)]
    pub api_backend: ApiBackend,
    /// Provider-private extensions, resolved by the sampler before requests.
    #[serde(default)]
    pub provider_extensions: ProviderExtensions,
    /// Extra headers to send with requests (e.g., for BYOK scenarios).''',
    )


def apply_sampler_config() -> None:
    path = "crates/codegen/xai-grok-sampler/src/config.rs"
    replace_exact(
        path,
        '''use xai_grok_sampling_types::{
    ApiBackend, CompactionAtTokens, CompactionsRemaining, DoomLoopRecoveryPolicy, ReasoningEffort,
};''',
        '''use xai_grok_sampling_types::{
    ApiBackend, CompactionAtTokens, CompactionsRemaining, DoomLoopRecoveryPolicy,
    ProviderExtensions, ReasoningEffort,
};''',
    )
    replace_exact(
        path,
        '''    pub api_backend: ApiBackend,
    #[serde(default)]
    pub auth_scheme: AuthScheme,''',
        '''    pub api_backend: ApiBackend,
    #[serde(default)]
    pub provider_extensions: ProviderExtensions,
    #[serde(default)]
    pub auth_scheme: AuthScheme,''',
    )
    replace_exact(
        path,
        '''            api_backend: ApiBackend::default(),
            auth_scheme: AuthScheme::default(),''',
        '''            api_backend: ApiBackend::default(),
            provider_extensions: ProviderExtensions::default(),
            auth_scheme: AuthScheme::default(),''',
    )


def apply_model_config() -> None:
    path = "crates/codegen/xai-grok-shell/src/agent/config.rs"
    replace_exact(
        path,
        '''    REASONING_EFFORTS_META_KEY, ReasoningEffort, ReasoningEffortOption,
    reasoning_effort_meta_value, reasoning_efforts_meta_value,''',
        '''    REASONING_EFFORTS_META_KEY, ProviderExtensions, ReasoningEffort,
    ReasoningEffortOption, reasoning_effort_meta_value, reasoning_efforts_meta_value,''',
    )
    replace_exact(
        path,
        '''    api_backend: ApiBackend,
    #[serde(default = "default_agent_type")]
    agent_type: String,''',
        '''    api_backend: ApiBackend,
    #[serde(default)]
    provider_extensions: ProviderExtensions,
    #[serde(default = "default_agent_type")]
    agent_type: String,''',
    )
    replace_exact(
        path,
        '''                api_backend: m.api_backend,
                auth_scheme: None,''',
        '''                api_backend: m.api_backend,
                provider_extensions: m.provider_extensions,
                auth_scheme: None,''',
    )
    replace_exact(
        path,
        '''    #[serde(default)]
    pub api_backend: ApiBackend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_scheme: Option<AuthScheme>,''',
        '''    #[serde(default)]
    pub api_backend: ApiBackend,
    /// Provider-private extensions independent from the API protocol shape.
    #[serde(default)]
    pub provider_extensions: ProviderExtensions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_scheme: Option<AuthScheme>,''',
    )
    replace_exact(
        path,
        '''    pub api_backend: Option<ApiBackend>,
    #[serde(default)]
    pub extra_headers: IndexMap<String, String>,''',
        '''    pub api_backend: Option<ApiBackend>,
    pub provider_extensions: Option<ProviderExtensions>,
    #[serde(default)]
    pub extra_headers: IndexMap<String, String>,''',
    )
    replace_exact(
        path,
        '''        if let Some(ref v) = self.api_backend {
            entry.info.api_backend = v.clone();
        }
        if !self.extra_headers.is_empty() {''',
        '''        if let Some(ref v) = self.api_backend {
            entry.info.api_backend = v.clone();
        }
        if let Some(v) = self.provider_extensions {
            entry.info.provider_extensions = v;
        }
        if !self.extra_headers.is_empty() {''',
    )
    replace_exact(
        path,
        '''    pub api_backend: ApiBackend,
    pub auth_scheme: AuthScheme,''',
        '''    pub api_backend: ApiBackend,
    pub provider_extensions: ProviderExtensions,
    pub auth_scheme: AuthScheme,''',
    )
    replace_exact(
        path,
        '''            api_backend: ApiBackend::default(),
            auth_scheme: Default::default(),''',
        '''            api_backend: ApiBackend::default(),
            provider_extensions: ProviderExtensions::default(),
            auth_scheme: Default::default(),''',
    )
    replace_exact(
        path,
        '''            api_backend: entry.api_backend.clone(),
            auth_scheme: entry.auth_scheme.unwrap_or_default(),''',
        '''            api_backend: entry.api_backend.clone(),
            provider_extensions: entry.provider_extensions,
            auth_scheme: entry.auth_scheme.unwrap_or_default(),''',
    )


def apply_sampler_client() -> None:
    path = "crates/codegen/xai-grok-sampler/src/client.rs"
    replace_exact(
        path,
        '''    ResponseModelMetadata, Result, SamplingError, build_messages_request, is_check_event, messages,
    rs,
};''',
        '''    ProviderExtensions, ResponseModelMetadata, Result, SamplingError, build_messages_request,
    is_check_event, messages, rs,
};''',
    )
    replace_exact(
        path,
        '''/// Process-level fallback for the `x-grok-client-identifier` header.
const DEFAULT_CLIENT_IDENTIFIER: &str = "grok-shell";

/// Product identifier baked into User-Agent strings.
const AGENT_PRODUCT: &str = "grok-shell";''',
        '''/// Process-level fallback for the xAI compatibility client identifier.
const DEFAULT_XAI_CLIENT_IDENTIFIER: &str = "dttn-cli";

/// Product identifier baked into User-Agent strings.
const AGENT_PRODUCT: &str = "dttn-cli";''',
    )
    replace_exact(
        path,
        '''    api_backend: ApiBackend,
    auth_scheme: AuthScheme,''',
        '''    api_backend: ApiBackend,
    provider_extensions: ProviderExtensions,
    auth_scheme: AuthScheme,''',
    )
    replace_exact(
        path,
        '''// =============================================================================
// SamplingClient
// =============================================================================

impl SamplingClient {''',
        '''// =============================================================================
// SamplingClient
// =============================================================================

fn resolve_provider_extensions(mode: ProviderExtensions, base_url: &str) -> ProviderExtensions {
    if !matches!(mode, ProviderExtensions::Auto) {
        return mode;
    }
    let is_xai = reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| {
            host == "x.ai"
                || host.ends_with(".x.ai")
                || host == "grok.com"
                || host.ends_with(".grok.com")
        });
    if is_xai {
        ProviderExtensions::Xai
    } else {
        ProviderExtensions::Standard
    }
}

fn validate_provider_extensions(
    config: &SamplerConfig,
    resolved: ProviderExtensions,
) -> Result<()> {
    if resolved.uses_xai() {
        return Ok(());
    }
    if config.stream_tool_calls {
        return Err(SamplingError::InvalidConfiguration(
            "stream_tool_calls requires provider_extensions = xai",
        ));
    }
    if config.supports_backend_search {
        return Err(SamplingError::InvalidConfiguration(
            "backend search requires provider_extensions = xai",
        ));
    }
    if config.compactions_remaining.is_some() || config.compaction_at_tokens.is_some() {
        return Err(SamplingError::InvalidConfiguration(
            "server compaction headers require provider_extensions = xai",
        ));
    }
    if config.doom_loop_recovery.is_some() {
        return Err(SamplingError::InvalidConfiguration(
            "server doom-loop recovery requires provider_extensions = xai",
        ));
    }
    if config.extra_headers.keys().any(|name| {
        name.eq_ignore_ascii_case("x-compaction-at")
            || name.eq_ignore_ascii_case("x-compactions-remaining")
            || name.to_ascii_lowercase().starts_with("x-grok-")
    }) {
        return Err(SamplingError::InvalidConfiguration(
            "xAI private headers require provider_extensions = xai",
        ));
    }
    Ok(())
}

impl SamplingClient {''',
    )
    replace_exact(
        path,
        '''    pub fn new(config: SamplerConfig) -> Result<Self> {
        let mut headers = HeaderMap::new();''',
        '''    pub fn new(config: SamplerConfig) -> Result<Self> {
        let provider_extensions =
            resolve_provider_extensions(config.provider_extensions, &config.base_url);
        validate_provider_extensions(&config, provider_extensions)?;

        let mut headers = HeaderMap::new();''',
    )
    replace_exact(
        path,
        '''        // Add x-grok-client-version header for version gating at the proxy.
        if let Some(client_version) = config.client_version.as_ref()
            && let Ok(header_value) = HeaderValue::from_str(client_version)
        {
            headers.insert(
                HeaderName::from_static("x-grok-client-version"),
                header_value,
            );
        }

        if let Some(deployment_id) = config.deployment_id.as_ref()
            && let Ok(header_value) = HeaderValue::from_str(deployment_id)
        {
            headers.insert(
                HeaderName::from_static("x-grok-deployment-id"),
                header_value,
            );
        }

        if let Some(user_id) = config.user_id.as_ref()
            && let Ok(header_value) = HeaderValue::from_str(user_id)
        {
            headers.insert(HeaderName::from_static("x-grok-user-id"), header_value);
        }

        {
            let client_id = config
                .client_identifier
                .clone()
                .unwrap_or_else(|| DEFAULT_CLIENT_IDENTIFIER.to_string());
            if let Ok(header_value) = HeaderValue::from_str(&client_id) {
                headers.insert(
                    HeaderName::from_static("x-grok-client-identifier"),
                    header_value,
                );
            }
        }''',
        '''        if provider_extensions.uses_xai() {
            // xAI compatibility headers are never sent to standard providers.
            if let Some(client_version) = config.client_version.as_ref()
                && let Ok(header_value) = HeaderValue::from_str(client_version)
            {
                headers.insert(
                    HeaderName::from_static("x-grok-client-version"),
                    header_value,
                );
            }

            if let Some(deployment_id) = config.deployment_id.as_ref()
                && let Ok(header_value) = HeaderValue::from_str(deployment_id)
            {
                headers.insert(
                    HeaderName::from_static("x-grok-deployment-id"),
                    header_value,
                );
            }

            if let Some(user_id) = config.user_id.as_ref()
                && let Ok(header_value) = HeaderValue::from_str(user_id)
            {
                headers.insert(HeaderName::from_static("x-grok-user-id"), header_value);
            }

            let client_id = config
                .client_identifier
                .clone()
                .unwrap_or_else(|| DEFAULT_XAI_CLIENT_IDENTIFIER.to_string());
            if let Ok(header_value) = HeaderValue::from_str(&client_id) {
                headers.insert(
                    HeaderName::from_static("x-grok-client-identifier"),
                    header_value,
                );
            }
        }''',
    )
    replace_exact(
        path,
        '''            api_backend = ?config.api_backend,
            auth_scheme = ?config.auth_scheme,''',
        '''            api_backend = ?config.api_backend,
            provider_extensions = ?provider_extensions,
            auth_scheme = ?config.auth_scheme,''',
    )
    replace_exact(
        path,
        '''            api_backend: config.api_backend,
            auth_scheme: config.auth_scheme,''',
        '''            api_backend: config.api_backend,
            provider_extensions,
            auth_scheme: config.auth_scheme,''',
    )
    replace_exact(
        path,
        '''    /// POST with default headers. Overrides auth from resolver if wired.
    fn post(&self, url: impl reqwest::IntoUrl) -> reqwest::RequestBuilder {''',
        '''    fn apply_provider_headers(
        &self,
        builder: reqwest::RequestBuilder,
        headers: &GrokRequestHeaders<'_>,
    ) -> reqwest::RequestBuilder {
        if self.defaults.provider_extensions.uses_xai() {
            headers.apply(builder)
        } else {
            builder
        }
    }

    /// POST with default headers. Overrides auth from resolver if wired.
    fn post(&self, url: impl reqwest::IntoUrl) -> reqwest::RequestBuilder {''',
    )
    replace_exact(
        path,
        '''        req_headers.push(Self::format_header("x-grok-conv-id", x_grok_conv_id));
        req_headers.push(Self::format_header("x-grok-req-id", x_grok_req_id));
        req_headers.push(Self::format_header("x-grok-model-override", model_id));''',
        '''        if self.defaults.provider_extensions.uses_xai() {
            req_headers.push(Self::format_header("x-grok-conv-id", x_grok_conv_id));
            req_headers.push(Self::format_header("x-grok-req-id", x_grok_req_id));
            req_headers.push(Self::format_header("x-grok-model-override", model_id));
        }''',
    )

    text = read(path)
    replacements = {
        '''let http_request = grok_headers
            .apply(self.post(self.endpoint("chat/completions")))''': '''let http_request = self.apply_provider_headers(
            self.post(self.endpoint("chat/completions")),
            &grok_headers,
        )''',
        '''let http_request = grok_headers
            .apply(self.post(self.endpoint("responses")))''': '''let http_request = self.apply_provider_headers(
            self.post(self.endpoint("responses")),
            &grok_headers,
        )''',
        '''let mut http_request = grok_headers
            .apply(self.post(self.endpoint("responses")))''': '''let mut http_request = self.apply_provider_headers(
            self.post(self.endpoint("responses")),
            &grok_headers,
        )''',
        '''let http_request = grok_headers
            .apply(self.post(self.endpoint("messages")))''': '''let http_request = self.apply_provider_headers(
            self.post(self.endpoint("messages")),
            &grok_headers,
        )''',
    }
    for old, new in replacements.items():
        count = text.count(old)
        if count == 0 and new in text:
            continue
        if count == 0:
            raise RuntimeError(f"{path}: missing provider-header call site: {old!r}")
        text = text.replace(old, new)
    write(path, text)

    replace_exact(
        path,
        '''        // Inject xAI-specific fields not in async-openai's CreateResponse type.
        if self.defaults.stream_tool_calls {
            request_body["stream_tool_calls"] = serde_json::json!(true);
        }
        // Inject xAI-specific tools (e.g., x_search) that can't be expressed
        // via async_openai's rs::Tool enum.
        if !extra_raw_tools.is_empty() {
            if let Some(tools) = request_body.get_mut("tools").and_then(|v| v.as_array_mut()) {
                tools.extend(extra_raw_tools);
            } else {
                request_body["tools"] = serde_json::Value::Array(extra_raw_tools);
            }
        }''',
        '''        if !self.defaults.provider_extensions.uses_xai() && !extra_raw_tools.is_empty() {
            return Err(SamplingError::InvalidConfiguration(
                "raw provider tools require provider_extensions = xai",
            ));
        }
        // Inject private fields only for explicitly resolved xAI compatibility.
        if self.defaults.provider_extensions.uses_xai() {
            if self.defaults.stream_tool_calls {
                request_body["stream_tool_calls"] = serde_json::json!(true);
            }
            if !extra_raw_tools.is_empty() {
                if let Some(tools) = request_body.get_mut("tools").and_then(|v| v.as_array_mut()) {
                    tools.extend(extra_raw_tools);
                } else {
                    request_body["tools"] = serde_json::Value::Array(extra_raw_tools);
                }
            }
        }''',
    )


def apply_model_defaults() -> None:
    path = "crates/codegen/xai-grok-models/default_models.json"
    replace_exact(
        path,
        '''      "api_backend": "responses",
      "agent_type": "dttn-code-agent",''',
        '''      "api_backend": "responses",
      "provider_extensions": "standard",
      "agent_type": "dttn-code-agent",''',
    )
    replace_exact(path, '"supported_in_api": false', '"supported_in_api": true')


def apply_literal_updates() -> None:
    for path in sorted((ROOT / "crates" / "codegen").rglob("*.rs")):
        add_provider_field_to_literals(path, "SamplerConfig")
        add_provider_field_to_literals(path, "SamplingConfig")

    # Production propagation sites must carry the selected mode rather than defaulting.
    replacements = [
        (
            "crates/codegen/xai-grok-shell/src/agent/config.rs",
            "        api_backend,\n        provider_extensions: Default::default(),",
            "        api_backend,\n        provider_extensions: info.provider_extensions,",
        ),
        (
            "crates/codegen/xai-grok-shell/src/agent/subagent/mod.rs",
            "                api_backend: cfg.api_backend,\n                provider_extensions: Default::default(),",
            "                api_backend: cfg.api_backend,\n                provider_extensions: cfg.provider_extensions,",
        ),
        (
            "crates/codegen/xai-grok-shell/src/session/acp_session_impl/sampler_turn.rs",
            "            api_backend: cfg.api_backend,\n            provider_extensions: Default::default(),",
            "            api_backend: cfg.api_backend,\n            provider_extensions: cfg.provider_extensions,",
        ),
        (
            "crates/codegen/xai-grok-shell/src/session/acp_session_impl/model_switch.rs",
            "                api_backend: sampling_config.api_backend.clone(),\n                provider_extensions: Default::default(),",
            "                api_backend: sampling_config.api_backend.clone(),\n                provider_extensions: sampling_config.provider_extensions,",
        ),
        (
            "crates/codegen/xai-grok-shell/src/session/acp_session_impl/spawn.rs",
            "        api_backend: sampling_config.api_backend.clone(),\n        provider_extensions: Default::default(),",
            "        api_backend: sampling_config.api_backend.clone(),\n        provider_extensions: sampling_config.provider_extensions,",
        ),
    ]
    for path, old, new in replacements:
        replace_exact(path, old, new)


if __name__ == "__main__":
    apply_sampling_types()
    apply_sampler_config()
    apply_model_config()
    apply_sampler_client()
    apply_model_defaults()
    apply_literal_updates()
    print("DTTN Phase 3 provider compatibility migration applied")
