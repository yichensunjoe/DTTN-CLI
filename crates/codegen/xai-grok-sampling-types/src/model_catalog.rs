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
    /// Company-managed, authenticated model registry.
    EnterpriseRegistry,
    /// Metadata returned directly by the configured provider API.
    ProviderApi,
    /// Audited public registry data with recorded origin and revision.
    VerifiedPublicRegistry,
    /// Previously verified provider or registry metadata stored locally.
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
            Self::UserOverride => 60,
            Self::EnterpriseRegistry => 50,
            Self::ProviderApi => 40,
            Self::VerifiedPublicRegistry => 30,
            Self::Cache => 20,
            Self::BuiltIn => 10,
            Self::Unknown => 0,
        }
    }

    /// Return the less trusted of two sources.
    pub const fn weaker(self, other: Self) -> Self {
        if self.priority() <= other.priority() {
            self
        } else {
            other
        }
    }
}

/// A metadata value together with provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sourced<T> {
    pub value: T,
    pub source: MetadataSource,
    /// Redacted origin identifier, such as a provider hostname or registry URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Provider version, ETag, release date, or registry commit used for auditing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

impl<T> Sourced<T> {
    pub const fn new(value: T, source: MetadataSource) -> Self {
        Self {
            value,
            source,
            origin: None,
            revision: None,
        }
    }

    pub fn with_origin(mut self, origin: impl Into<String>) -> Self {
        self.origin = Some(origin.into());
        self
    }

    pub fn with_revision(mut self, revision: impl Into<String>) -> Self {
        self.revision = Some(revision.into());
        self
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
    /// Audited public registry mirror, never supplied with provider credentials.
    PublicRegistry,
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

/// Price fields are stored as micro-units of `currency` per one million tokens.
///
/// For example, USD 3.00 / 1M input tokens is represented as `3_000_000`
/// micro-USD. Integer storage avoids floating-point drift in long sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelPricing {
    pub currency: Option<Sourced<String>>,
    pub input_per_million_microunits: Option<Sourced<u64>>,
    pub cached_input_per_million_microunits: Option<Sourced<u64>>,
    pub output_per_million_microunits: Option<Sourced<u64>>,
    /// Only used when the provider reports reasoning tokens as separately billable.
    pub reasoning_per_million_microunits: Option<Sourced<u64>>,
}

/// Provider-normalized billable usage.
///
/// Adapters must not place reasoning tokens here when they are already included
/// in `output_tokens`; doing so would double-count cost.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillableTokenUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub separately_billable_reasoning_tokens: u64,
}

/// Deterministic local cost estimate based on token usage and resolved prices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostEstimate {
    pub currency: String,
    pub micro_units: u64,
    /// Weakest source among every price field used by the estimate.
    pub pricing_source: MetadataSource,
}

impl ModelPricing {
    /// Estimate cost only when every non-zero token bucket has a known price.
    ///
    /// Returning `None` is intentional: the status bar must hide unknown cost
    /// instead of presenting a partial total as authoritative.
    pub fn estimate_cost(&self, usage: BillableTokenUsage) -> Option<CostEstimate> {
        let currency = self.currency.as_ref()?;
        let currency_value = currency.value.trim();
        if currency_value.is_empty() {
            return None;
        }

        let mut numerator = 0u128;
        let mut weakest_source = currency.source;
        add_price_bucket(
            &mut numerator,
            &mut weakest_source,
            usage.input_tokens,
            self.input_per_million_microunits.as_ref(),
        )?;
        add_price_bucket(
            &mut numerator,
            &mut weakest_source,
            usage.cached_input_tokens,
            self.cached_input_per_million_microunits.as_ref(),
        )?;
        add_price_bucket(
            &mut numerator,
            &mut weakest_source,
            usage.output_tokens,
            self.output_per_million_microunits.as_ref(),
        )?;
        add_price_bucket(
            &mut numerator,
            &mut weakest_source,
            usage.separately_billable_reasoning_tokens,
            self.reasoning_per_million_microunits.as_ref(),
        )?;

        // Rates are per 1M tokens. Round up at the micro-currency boundary so a
        // non-zero billable request never disappears as a displayed zero.
        let rounded = numerator.checked_add(999_999)?.checked_div(1_000_000)?;
        let micro_units = u64::try_from(rounded).ok()?;
        Some(CostEstimate {
            currency: currency_value.to_owned(),
            micro_units,
            pricing_source: weakest_source,
        })
    }
}

fn add_price_bucket(
    numerator: &mut u128,
    weakest_source: &mut MetadataSource,
    tokens: u64,
    price: Option<&Sourced<u64>>,
) -> Option<()> {
    if tokens == 0 {
        return Some(());
    }
    let price = price?;
    let amount = (tokens as u128).checked_mul(price.value as u128)?;
    *numerator = numerator.checked_add(amount)?;
    *weakest_source = weakest_source.weaker(price.source);
    Some(())
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
    pub pricing: ModelPricing,
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
        merge_pricing(&mut self.pricing, candidate.pricing);
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

fn merge_pricing(current: &mut ModelPricing, candidate: ModelPricing) {
    merge_field(&mut current.currency, candidate.currency);
    merge_field(
        &mut current.input_per_million_microunits,
        candidate.input_per_million_microunits,
    );
    merge_field(
        &mut current.cached_input_per_million_microunits,
        candidate.cached_input_per_million_microunits,
    );
    merge_field(
        &mut current.output_per_million_microunits,
        candidate.output_per_million_microunits,
    );
    merge_field(
        &mut current.reasoning_per_million_microunits,
        candidate.reasoning_per_million_microunits,
    );
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
    fn verified_registry_cannot_replace_provider_api() {
        let mut resolved = metadata(MetadataSource::ProviderApi, 256_000);
        resolved
            .merge(metadata(
                MetadataSource::VerifiedPublicRegistry,
                128_000,
            ))
            .unwrap();
        assert_eq!(resolved.context_window.unwrap().value, 256_000);
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

    #[test]
    fn pricing_merge_is_field_level_and_source_aware() {
        let mut resolved = metadata(MetadataSource::BuiltIn, 128_000);
        resolved.pricing.currency = Some(Sourced::new(
            "USD".to_string(),
            MetadataSource::BuiltIn,
        ));
        resolved.pricing.input_per_million_microunits =
            Some(Sourced::new(3_000_000, MetadataSource::BuiltIn));

        let mut provider = metadata(MetadataSource::ProviderApi, 128_000);
        provider.pricing.input_per_million_microunits =
            Some(Sourced::new(2_000_000, MetadataSource::ProviderApi));
        provider.pricing.output_per_million_microunits =
            Some(Sourced::new(8_000_000, MetadataSource::ProviderApi));
        resolved.merge(provider).unwrap();

        assert_eq!(
            resolved
                .pricing
                .input_per_million_microunits
                .unwrap()
                .value,
            2_000_000
        );
        assert_eq!(
            resolved
                .pricing
                .output_per_million_microunits
                .unwrap()
                .value,
            8_000_000
        );
    }

    #[test]
    fn estimates_cost_without_floating_point_drift() {
        let pricing = ModelPricing {
            currency: Some(Sourced::new(
                "USD".to_string(),
                MetadataSource::ProviderApi,
            )),
            input_per_million_microunits: Some(Sourced::new(
                2_000_000,
                MetadataSource::ProviderApi,
            )),
            cached_input_per_million_microunits: Some(Sourced::new(
                500_000,
                MetadataSource::ProviderApi,
            )),
            output_per_million_microunits: Some(Sourced::new(
                8_000_000,
                MetadataSource::ProviderApi,
            )),
            reasoning_per_million_microunits: None,
        };
        let estimate = pricing
            .estimate_cost(BillableTokenUsage {
                input_tokens: 1_000_000,
                cached_input_tokens: 2_000_000,
                output_tokens: 500_000,
                separately_billable_reasoning_tokens: 0,
            })
            .unwrap();

        assert_eq!(estimate.currency, "USD");
        assert_eq!(estimate.micro_units, 7_000_000);
        assert_eq!(estimate.pricing_source, MetadataSource::ProviderApi);
    }

    #[test]
    fn unknown_non_zero_price_bucket_hides_cost() {
        let pricing = ModelPricing {
            currency: Some(Sourced::new(
                "USD".to_string(),
                MetadataSource::ProviderApi,
            )),
            input_per_million_microunits: Some(Sourced::new(
                2_000_000,
                MetadataSource::ProviderApi,
            )),
            ..Default::default()
        };

        assert!(
            pricing
                .estimate_cost(BillableTokenUsage {
                    input_tokens: 1_000,
                    output_tokens: 1_000,
                    ..Default::default()
                })
                .is_none()
        );
    }
}
