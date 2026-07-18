//! Provider-neutral model metadata used by DTTN model discovery.
//!
//! This module intentionally contains no network or filesystem I/O. Provider
//! adapters return partial [`ModelMetadata`] values and the runtime merges them
//! with an explicit precedence order. A field is never silently replaced by a
//! lower-confidence source.

use serde::{Deserialize, Serialize};

/// Where one model metadata value came from.
///
/// The declaration order is not the precedence order. Use [`MetadataSource::priority`]
/// when resolving conflicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataSource {
    /// User or administrator configuration. Always authoritative.
    UserOverride,
    /// Company-managed, signed model registry.
    EnterpriseRegistry,
    /// Metadata returned directly by the configured provider API.
    ProviderApi,
    /// Previously verified provider metadata stored locally.
    Cache,
    /// Distribution defaults shipped with DTTN.
    BuiltIn,
    /// Unknown origin. Accepted only when no stronger value exists.
    Unknown,
}

impl MetadataSource {
    /// Larger values win during metadata merging.
    pub const fn priority(self) -> u8 {
        match self {
            Self::UserOverride => 50,
            Self::EnterpriseRegistry => 40,
            Self::ProviderApi => 30,
            Self::Cache => 20,
            Self::BuiltIn => 10,
            Self::Unknown => 0,
        }
    }
}

/// A metadata value together with provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sourced<T> {
    pub value: T,
    pub source: MetadataSource,
}

impl<T> Sourced<T> {
    pub const fn new(value: T, source: MetadataSource) -> Self {
        Self { value, source }
    }
}

/// Provider protocol used to invoke a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelProtocol {
    ChatCompletions,
    Responses,
    AnthropicMessages,
    GeminiGenerateContent,
}

/// Provider API shape used to discover model metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogProtocol {
    /// OpenAI-compatible `/v1/models`; usually provides identity only.
    OpenAiModels,
    /// Gemini `models.get/list`; includes input/output token limits.
    GeminiModels,
    /// Mistral `/v1/models`; includes context length and capabilities.
    MistralModels,
    /// Anthropic models endpoint. Capability fields may require registry enrichment.
    AnthropicModels,
    /// Company-owned DTTN model registry schema.
    DttnRegistry,
    /// No remote discovery. Use registry/cache/overrides only.
    Static,
}

/// Capabilities that materially affect Agent Harness behavior.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub tool_calling: Option<Sourced<bool>>,
    pub parallel_tool_calls: Option<Sourced<bool>>,
    pub vision: Option<Sourced<bool>>,
    pub reasoning: Option<Sourced<bool>>,
    pub strict_json_schema: Option<Sourced<bool>>,
    pub streaming: Option<Sourced<bool>>,
}

/// Partial metadata for one routing model.
///
/// `context_window` is the total model context budget. `max_input_tokens` and
/// `max_output_tokens` are kept separately because some providers expose them
/// independently and because `input + output == context` is not universal.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelMetadata {
    pub model_id: String,
    pub display_name: Option<Sourced<String>>,
    pub protocol: Option<Sourced<ModelProtocol>>,
    pub context_window: Option<Sourced<u64>>,
    pub max_input_tokens: Option<Sourced<u64>>,
    pub max_output_tokens: Option<Sourced<u64>>,
    pub default_temperature: Option<Sourced<f32>>,
    pub max_temperature: Option<Sourced<f32>>,
    pub capabilities: ModelCapabilities,
}

impl ModelMetadata {
    /// Merge `candidate` into `self`, keeping the value with the strongest source.
    ///
    /// Model IDs must match. This makes alias resolution an explicit operation in
    /// the provider adapter rather than an accidental side effect of merging.
    pub fn merge(&mut self, candidate: Self) -> Result<(), ModelMetadataMergeError> {
        if self.model_id != candidate.model_id {
            return Err(ModelMetadataMergeError::ModelIdMismatch {
                expected: self.model_id.clone(),
                received: candidate.model_id,
            });
        }

        merge_field(&mut self.display_name, candidate.display_name);
        merge_field(&mut self.protocol, candidate.protocol);
        merge_field(&mut self.context_window, candidate.context_window);
        merge_field(&mut self.max_input_tokens, candidate.max_input_tokens);
        merge_field(&mut self.max_output_tokens, candidate.max_output_tokens);
        merge_field(
            &mut self.default_temperature,
            candidate.default_temperature,
        );
        merge_field(&mut self.max_temperature, candidate.max_temperature);
        merge_capabilities(&mut self.capabilities, candidate.capabilities);
        Ok(())
    }
}

fn merge_field<T>(current: &mut Option<Sourced<T>>, candidate: Option<Sourced<T>>) {
    let Some(candidate) = candidate else {
        return;
    };
    if current
        .as_ref()
        .is_none_or(|existing| candidate.source.priority() > existing.source.priority())
    {
        *current = Some(candidate);
    }
}

fn merge_capabilities(current: &mut ModelCapabilities, candidate: ModelCapabilities) {
    merge_field(&mut current.tool_calling, candidate.tool_calling);
    merge_field(
        &mut current.parallel_tool_calls,
        candidate.parallel_tool_calls,
    );
    merge_field(&mut current.vision, candidate.vision);
    merge_field(&mut current.reasoning, candidate.reasoning);
    merge_field(
        &mut current.strict_json_schema,
        candidate.strict_json_schema,
    );
    merge_field(&mut current.streaming, candidate.streaming);
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelMetadataMergeError {
    #[error("model metadata ID mismatch: expected {expected}, received {received}")]
    ModelIdMismatch { expected: String, received: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(source: MetadataSource, context: u64) -> ModelMetadata {
        ModelMetadata {
            model_id: "example-model".to_string(),
            context_window: Some(Sourced::new(context, source)),
            ..Default::default()
        }
    }

    #[test]
    fn higher_priority_source_replaces_lower_priority_value() {
        let mut resolved = metadata(MetadataSource::BuiltIn, 128_000);
        resolved
            .merge(metadata(MetadataSource::ProviderApi, 256_000))
            .unwrap();
        assert_eq!(resolved.context_window.unwrap().value, 256_000);
    }

    #[test]
    fn lower_priority_source_cannot_replace_override() {
        let mut resolved = metadata(MetadataSource::UserOverride, 512_000);
        resolved
            .merge(metadata(MetadataSource::ProviderApi, 256_000))
            .unwrap();
        let context = resolved.context_window.unwrap();
        assert_eq!(context.value, 512_000);
        assert_eq!(context.source, MetadataSource::UserOverride);
    }

    #[test]
    fn equal_priority_keeps_first_value_for_determinism() {
        let mut resolved = metadata(MetadataSource::ProviderApi, 256_000);
        resolved
            .merge(metadata(MetadataSource::ProviderApi, 128_000))
            .unwrap();
        assert_eq!(resolved.context_window.unwrap().value, 256_000);
    }

    #[test]
    fn merge_rejects_different_model_ids() {
        let mut resolved = metadata(MetadataSource::BuiltIn, 128_000);
        let mut other = metadata(MetadataSource::ProviderApi, 256_000);
        other.model_id = "other-model".to_string();
        assert!(matches!(
            resolved.merge(other),
            Err(ModelMetadataMergeError::ModelIdMismatch { .. })
        ));
    }
}
