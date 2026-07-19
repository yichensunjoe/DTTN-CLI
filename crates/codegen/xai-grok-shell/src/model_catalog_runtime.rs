//! Provider-neutral model discovery parsing and credential-free sidecar caching.
//!
//! This module does not change model selection. It normalizes explicit provider
//! fields into sourced metadata and stores validated snapshots for later Doctor,
//! compaction, pricing, and status-line integration.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use xai_grok_sampling_types::{
    MetadataSource, ModelCapabilities, ModelMetadata, ModelPricing, ModelProtocol, Sourced,
};

pub const MODEL_CATALOG_CACHE_SCHEMA_VERSION: u32 = 1;
const MAX_CACHE_BYTES: usize = 4 * 1024 * 1024;
const MAX_CATALOG_MODELS: usize = 10_000;
const DEFAULT_CACHE_ENTRIES: usize = 3;
static CACHE_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogParseOptions<'a> {
    pub origin: &'a str,
    pub revision: Option<&'a str>,
    pub default_protocol: ModelProtocol,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogParseError {
    #[error("catalog root must be a JSON object")]
    RootNotObject,
    #[error("catalog field '{field}' must be an array")]
    ModelsNotArray { field: &'static str },
    #[error("unsupported DTTN registry schema version: {0}")]
    UnsupportedSchema(u64),
    #[error("model entry at index {index} is not an object")]
    ModelNotObject { index: usize },
    #[error("model entry at index {index} has no id/model field")]
    MissingModelId { index: usize },
    #[error("catalog contains duplicate model id '{0}'")]
    DuplicateModelId(String),
    #[error("catalog contains more than {MAX_CATALOG_MODELS} models")]
    TooManyModels,
}

/// Parse an OpenAI-compatible `GET /v1/models` response.
///
/// Standard identity-only responses remain identity-only. Limits, capabilities,
/// and pricing are consumed only from explicit fields, never from model-name
/// heuristics.
pub fn parse_openai_compatible_catalog(
    payload: &Value,
    options: &CatalogParseOptions<'_>,
) -> Result<Vec<ModelMetadata>, CatalogParseError> {
    parse_catalog_array(payload, "data", MetadataSource::ProviderApi, options)
}

/// Parse the normalized company-owned DTTN registry schema.
pub fn parse_dttn_registry_catalog(
    payload: &Value,
    options: &CatalogParseOptions<'_>,
) -> Result<Vec<ModelMetadata>, CatalogParseError> {
    let root = payload
        .as_object()
        .ok_or(CatalogParseError::RootNotObject)?;
    let schema = root
        .get("schema_version")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if schema != MODEL_CATALOG_CACHE_SCHEMA_VERSION as u64 {
        return Err(CatalogParseError::UnsupportedSchema(schema));
    }
    parse_catalog_array(
        payload,
        "models",
        MetadataSource::EnterpriseRegistry,
        options,
    )
}

fn parse_catalog_array(
    payload: &Value,
    array_key: &'static str,
    source: MetadataSource,
    options: &CatalogParseOptions<'_>,
) -> Result<Vec<ModelMetadata>, CatalogParseError> {
    let root = payload
        .as_object()
        .ok_or(CatalogParseError::RootNotObject)?;
    let entries = root
        .get(array_key)
        .and_then(Value::as_array)
        .ok_or(CatalogParseError::ModelsNotArray { field: array_key })?;
    if entries.len() > MAX_CATALOG_MODELS {
        return Err(CatalogParseError::TooManyModels);
    }

    let mut seen = HashSet::with_capacity(entries.len());
    let mut models = Vec::with_capacity(entries.len());
    for (index, value) in entries.iter().enumerate() {
        let object = value
            .as_object()
            .ok_or(CatalogParseError::ModelNotObject { index })?;
        let model_id = lookup_string(object, &["id", "model", "modelId"])
            .ok_or(CatalogParseError::MissingModelId { index })?;
        if !seen.insert(model_id.clone()) {
            return Err(CatalogParseError::DuplicateModelId(model_id));
        }
        models.push(parse_model(object, model_id, source, options));
    }
    Ok(models)
}

fn parse_model(
    object: &Map<String, Value>,
    model_id: String,
    source: MetadataSource,
    options: &CatalogParseOptions<'_>,
) -> ModelMetadata {
    let protocol = lookup_string(
        object,
        &["protocol", "api_backend", "apiBackend", "endpoint_type"],
    )
    .and_then(|value| parse_protocol(&value))
    .unwrap_or(options.default_protocol);

    ModelMetadata {
        model_id,
        display_name: lookup_string(object, &["display_name", "displayName", "name"])
            .map(|value| sourced(value, source, options)),
        protocol: Some(sourced(protocol, source, options)),
        context_window: lookup_u64(
            object,
            &[
                "context_window",
                "contextWindow",
                "max_context_length",
                "maxContextLength",
                "totalContextTokens",
            ],
        )
        .map(|value| sourced(value, source, options)),
        max_input_tokens: lookup_u64(
            object,
            &["max_input_tokens", "maxInputTokens", "inputTokenLimit"],
        )
        .map(|value| sourced(value, source, options)),
        max_output_tokens: lookup_u64(
            object,
            &[
                "max_output_tokens",
                "maxOutputTokens",
                "max_completion_tokens",
                "maxCompletionTokens",
                "outputTokenLimit",
            ],
        )
        .map(|value| sourced(value, source, options)),
        default_temperature: lookup_f64(
            object,
            &["default_temperature", "defaultTemperature", "temperature"],
        )
        .map(|value| sourced(value as f32, source, options)),
        max_temperature: lookup_f64(object, &["max_temperature", "maxTemperature"])
            .map(|value| sourced(value as f32, source, options)),
        capabilities: parse_capabilities(object, source, options),
        pricing: parse_pricing(object, source, options),
    }
}

fn parse_capabilities(
    object: &Map<String, Value>,
    source: MetadataSource,
    options: &CatalogParseOptions<'_>,
) -> ModelCapabilities {
    let nested = object.get("capabilities").and_then(Value::as_object);
    let boolean = |names: &[&str]| {
        lookup_bool_in(object, nested, names).map(|value| sourced(value, source, options))
    };
    ModelCapabilities {
        tool_calling: boolean(&[
            "tool_calling",
            "toolCalling",
            "function_calling",
            "functionCalling",
            "supportsFunctionCalling",
        ]),
        parallel_tool_calls: boolean(&[
            "parallel_tool_calls",
            "parallelToolCalls",
            "supportsParallelToolCalls",
        ]),
        vision: boolean(&["vision", "supportsVision"]),
        reasoning: boolean(&["reasoning", "supportsReasoning"]),
        strict_json_schema: boolean(&[
            "strict_json_schema",
            "strictJsonSchema",
            "supportsStrictJsonSchema",
        ]),
        streaming: boolean(&["streaming", "supportsStreaming"]),
    }
}

fn parse_pricing(
    object: &Map<String, Value>,
    source: MetadataSource,
    options: &CatalogParseOptions<'_>,
) -> ModelPricing {
    let nested = object.get("pricing").and_then(Value::as_object);
    let string = |names: &[&str]| {
        lookup_string_in(object, nested, names).map(|value| sourced(value, source, options))
    };
    let integer = |names: &[&str]| {
        lookup_u64_in(object, nested, names).map(|value| sourced(value, source, options))
    };

    // Only explicit integer micro-unit fields are accepted. Ambiguous provider
    // fields such as `input_price: 3.0` are not converted because their units
    // differ across providers.
    ModelPricing {
        currency: string(&["currency", "currency_code", "currencyCode"]),
        input_per_million_microunits: integer(&[
            "input_per_million_microunits",
            "inputPerMillionMicrounits",
        ]),
        cached_input_per_million_microunits: integer(&[
            "cached_input_per_million_microunits",
            "cachedInputPerMillionMicrounits",
        ]),
        output_per_million_microunits: integer(&[
            "output_per_million_microunits",
            "outputPerMillionMicrounits",
        ]),
        reasoning_per_million_microunits: integer(&[
            "reasoning_per_million_microunits",
            "reasoningPerMillionMicrounits",
        ]),
    }
}

fn parse_protocol(value: &str) -> Option<ModelProtocol> {
    match value.trim().to_ascii_lowercase().as_str() {
        "chat_completions" | "chat-completions" | "chat" => Some(ModelProtocol::ChatCompletions),
        "responses" | "response" => Some(ModelProtocol::Responses),
        "messages" | "anthropic_messages" | "anthropic-messages" => {
            Some(ModelProtocol::AnthropicMessages)
        }
        "gemini_generate_content" | "gemini-generate-content" | "generate_content" => {
            Some(ModelProtocol::GeminiGenerateContent)
        }
        _ => None,
    }
}

fn sourced<T>(value: T, source: MetadataSource, options: &CatalogParseOptions<'_>) -> Sourced<T> {
    let mut value = Sourced::new(value, source).with_origin(options.origin);
    if let Some(revision) = options.revision {
        value = value.with_revision(revision);
    }
    value
}

fn lookup_string(object: &Map<String, Value>, names: &[&str]) -> Option<String> {
    lookup_string_in(
        object,
        object.get("_meta").and_then(Value::as_object),
        names,
    )
}

fn lookup_string_in(
    object: &Map<String, Value>,
    nested: Option<&Map<String, Value>>,
    names: &[&str],
) -> Option<String> {
    find_value(object, nested, names, Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn lookup_u64(object: &Map<String, Value>, names: &[&str]) -> Option<u64> {
    lookup_u64_in(
        object,
        object.get("_meta").and_then(Value::as_object),
        names,
    )
}

fn lookup_u64_in(
    object: &Map<String, Value>,
    nested: Option<&Map<String, Value>>,
    names: &[&str],
) -> Option<u64> {
    names
        .iter()
        .find_map(|name| object.get(*name).and_then(value_as_u64))
        .or_else(|| {
            nested.and_then(|nested| {
                names
                    .iter()
                    .find_map(|name| nested.get(*name).and_then(value_as_u64))
            })
        })
}

fn lookup_f64(object: &Map<String, Value>, names: &[&str]) -> Option<f64> {
    find_value(
        object,
        object.get("_meta").and_then(Value::as_object),
        names,
        Value::as_f64,
    )
}

fn lookup_bool_in(
    object: &Map<String, Value>,
    nested: Option<&Map<String, Value>>,
    names: &[&str],
) -> Option<bool> {
    find_value(object, nested, names, Value::as_bool)
}

fn find_value<'a, T>(
    object: &'a Map<String, Value>,
    nested: Option<&'a Map<String, Value>>,
    names: &[&str],
    convert: impl Fn(&'a Value) -> Option<T> + Copy,
) -> Option<T> {
    names
        .iter()
        .find_map(|name| object.get(*name).and_then(convert))
        .or_else(|| {
            nested.and_then(|nested| {
                names
                    .iter()
                    .find_map(|name| nested.get(*name).and_then(convert))
            })
        })
}

fn value_as_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCatalogCacheDocument {
    pub schema_version: u32,
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    pub fetched_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub models: Vec<ModelMetadata>,
}

impl ModelCatalogCacheDocument {
    pub fn new(
        origin: impl Into<String>,
        revision: Option<String>,
        fetched_at_unix_ms: u64,
        expires_at_unix_ms: u64,
        models: Vec<ModelMetadata>,
    ) -> Self {
        Self {
            schema_version: MODEL_CATALOG_CACHE_SCHEMA_VERSION,
            origin: origin.into(),
            revision,
            fetched_at_unix_ms,
            expires_at_unix_ms,
            models,
        }
    }

    pub fn is_stale(&self, now_unix_ms: u64) -> bool {
        now_unix_ms >= self.expires_at_unix_ms
    }

    pub fn validate(&self) -> Result<(), ModelCatalogCacheError> {
        if self.schema_version != MODEL_CATALOG_CACHE_SCHEMA_VERSION {
            return Err(ModelCatalogCacheError::Invalid(format!(
                "unsupported schema version {}",
                self.schema_version
            )));
        }
        if self.origin.trim().is_empty() {
            return Err(ModelCatalogCacheError::Invalid(
                "cache origin must not be blank".to_string(),
            ));
        }
        if self.expires_at_unix_ms < self.fetched_at_unix_ms {
            return Err(ModelCatalogCacheError::Invalid(
                "cache expiry precedes fetch time".to_string(),
            ));
        }
        if self.models.len() > MAX_CATALOG_MODELS {
            return Err(ModelCatalogCacheError::Invalid(format!(
                "catalog contains more than {MAX_CATALOG_MODELS} models"
            )));
        }

        let mut ids = HashSet::with_capacity(self.models.len());
        for model in &self.models {
            if model.model_id.trim().is_empty() {
                return Err(ModelCatalogCacheError::Invalid(
                    "model id must not be blank".to_string(),
                ));
            }
            if !ids.insert(model.model_id.as_str()) {
                return Err(ModelCatalogCacheError::Invalid(format!(
                    "duplicate model id '{}'",
                    model.model_id
                )));
            }
            if model
                .context_window
                .as_ref()
                .is_some_and(|value| value.value == 0)
            {
                return Err(ModelCatalogCacheError::Invalid(format!(
                    "model '{}' has a zero context window",
                    model.model_id
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogFreshness {
    Fresh,
    Stale,
}

#[derive(Debug, Clone)]
pub struct CachedModelCatalog {
    pub document: ModelCatalogCacheDocument,
    pub freshness: CatalogFreshness,
    pub path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum ModelCatalogCacheError {
    #[error("model catalog cache I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("model catalog cache serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("model catalog cache is invalid: {0}")]
    Invalid(String),
}

/// Append-only cache directory using temporary-file to unique-final-file rename.
///
/// The destination never exists, so the commit is atomic on Windows and Unix.
/// Readers ignore `.tmp` files and fall back to the newest older valid entry.
#[derive(Debug, Clone)]
pub struct ModelCatalogCache {
    directory: PathBuf,
    max_entries: usize,
}

impl ModelCatalogCache {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            max_entries: DEFAULT_CACHE_ENTRIES,
        }
    }

    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries.max(1);
        self
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn store(
        &self,
        document: &ModelCatalogCacheDocument,
    ) -> Result<PathBuf, ModelCatalogCacheError> {
        document.validate()?;
        let json = serde_json::to_vec_pretty(document)?;
        if json.len() > MAX_CACHE_BYTES {
            return Err(ModelCatalogCacheError::Invalid(format!(
                "serialized catalog exceeds {MAX_CACHE_BYTES} bytes"
            )));
        }

        fs::create_dir_all(&self.directory)?;
        let nonce = CACHE_NONCE.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let final_name = format!(
            "catalog-{:020}-{pid:010}-{nonce:020}.json",
            document.fetched_at_unix_ms
        );
        let temp_path = self.directory.join(format!(".{final_name}.tmp"));
        let final_path = self.directory.join(final_name);

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp_path)?;
        file.write_all(&json)?;
        file.sync_all()?;
        drop(file);
        if let Err(error) = fs::rename(&temp_path, &final_path) {
            let _ = fs::remove_file(&temp_path);
            return Err(error.into());
        }

        self.prune_old_entries();
        Ok(final_path)
    }

    pub fn load_latest(
        &self,
        expected_origin: Option<&str>,
        now_unix_ms: u64,
    ) -> Result<Option<CachedModelCatalog>, ModelCatalogCacheError> {
        let mut paths = match fs::read_dir(&self.directory) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| is_final_cache_path(path))
                .collect::<Vec<_>>(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        paths.sort_unstable_by(|left, right| right.file_name().cmp(&left.file_name()));

        for path in paths {
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => continue,
                Ok(metadata) if metadata.len() as usize > MAX_CACHE_BYTES => continue,
                Ok(_) => {}
                Err(_) => continue,
            };
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            let document = match serde_json::from_slice::<ModelCatalogCacheDocument>(&bytes) {
                Ok(document) => document,
                Err(_) => continue,
            };
            if document.validate().is_err() {
                continue;
            }
            if expected_origin.is_some_and(|origin| document.origin != origin) {
                continue;
            }
            let freshness = if document.is_stale(now_unix_ms) {
                CatalogFreshness::Stale
            } else {
                CatalogFreshness::Fresh
            };
            return Ok(Some(CachedModelCatalog {
                document,
                freshness,
                path,
            }));
        }
        Ok(None)
    }

    fn prune_old_entries(&self) {
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return;
        };
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| is_final_cache_path(path))
            .collect::<Vec<_>>();
        paths.sort_unstable_by(|left, right| right.file_name().cmp(&left.file_name()));
        for path in paths.into_iter().skip(self.max_entries) {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn default_model_catalog_cache() -> ModelCatalogCache {
    ModelCatalogCache::new(
        crate::util::grok_home::grok_home()
            .join("cache")
            .join("model-catalog-v1"),
    )
}

fn is_final_cache_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("catalog-") && name.ends_with(".json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn options<'a>() -> CatalogParseOptions<'a> {
        CatalogParseOptions {
            origin: "https://provider.example/v1/models",
            revision: Some("etag-123"),
            default_protocol: ModelProtocol::ChatCompletions,
        }
    }

    #[test]
    fn parses_explicit_extensions_with_provenance() {
        let payload = json!({
            "data": [{
                "id": "example-model",
                "name": "Example Model",
                "context_window": 262144,
                "max_completion_tokens": "65536",
                "capabilities": {"tool_calling": true, "vision": true},
                "pricing": {
                    "currency": "USD",
                    "input_per_million_microunits": 2_000_000,
                    "output_per_million_microunits": 8_000_000
                }
            }]
        });

        let model = parse_openai_compatible_catalog(&payload, &options())
            .unwrap()
            .remove(0);
        assert_eq!(model.context_window.as_ref().unwrap().value, 262_144);
        assert_eq!(model.max_output_tokens.as_ref().unwrap().value, 65_536);
        assert_eq!(
            model.context_window.as_ref().unwrap().source,
            MetadataSource::ProviderApi
        );
        assert_eq!(
            model.context_window.as_ref().unwrap().origin.as_deref(),
            Some("https://provider.example/v1/models")
        );
        assert_eq!(
            model.pricing.output_per_million_microunits.unwrap().value,
            8_000_000
        );
    }

    #[test]
    fn identity_only_response_keeps_unknown_fields_unknown() {
        let payload = json!({"data": [{"id": "identity-only"}]});
        let model = parse_openai_compatible_catalog(&payload, &options())
            .unwrap()
            .remove(0);
        assert!(model.context_window.is_none());
        assert!(model.max_output_tokens.is_none());
        assert!(model.pricing.input_per_million_microunits.is_none());
        assert!(model.capabilities.tool_calling.is_none());
    }

    #[test]
    fn registry_requires_supported_schema_and_uses_enterprise_source() {
        let payload = json!({
            "schema_version": 1,
            "models": [{"id": "company/model", "context_window": 128000}]
        });
        let models = parse_dttn_registry_catalog(&payload, &options()).unwrap();
        assert_eq!(
            models[0].context_window.as_ref().unwrap().source,
            MetadataSource::EnterpriseRegistry
        );

        let unsupported = json!({"schema_version": 2, "models": []});
        assert!(matches!(
            parse_dttn_registry_catalog(&unsupported, &options()),
            Err(CatalogParseError::UnsupportedSchema(2))
        ));
    }

    #[test]
    fn rejects_duplicate_model_ids() {
        let payload = json!({"data": [{"id": "same"}, {"id": "same"}]});
        assert!(matches!(
            parse_openai_compatible_catalog(&payload, &options()),
            Err(CatalogParseError::DuplicateModelId(id)) if id == "same"
        ));
    }

    fn document(fetched: u64, expires: u64) -> ModelCatalogCacheDocument {
        ModelCatalogCacheDocument::new(
            "https://provider.example/v1/models",
            Some("etag-123".to_string()),
            fetched,
            expires,
            vec![ModelMetadata {
                model_id: "example-model".to_string(),
                context_window: Some(Sourced::new(262_144, MetadataSource::ProviderApi)),
                ..Default::default()
            }],
        )
    }

    #[test]
    fn cache_round_trip_reports_freshness_and_origin() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = ModelCatalogCache::new(dir.path());
        cache.store(&document(1_000, 2_000)).unwrap();

        let fresh = cache
            .load_latest(Some("https://provider.example/v1/models"), 1_500)
            .unwrap()
            .unwrap();
        assert_eq!(fresh.freshness, CatalogFreshness::Fresh);

        let stale = cache
            .load_latest(Some("https://provider.example/v1/models"), 2_000)
            .unwrap()
            .unwrap();
        assert_eq!(stale.freshness, CatalogFreshness::Stale);
        assert!(
            cache
                .load_latest(Some("https://other.example/v1/models"), 1_500)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn corrupt_newest_entry_falls_back_to_previous_valid_entry() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = ModelCatalogCache::new(dir.path()).with_max_entries(4);
        cache.store(&document(1_000, 3_000)).unwrap();
        fs::write(
            dir.path()
                .join("catalog-00000000000000002000-0000000001-00000000000000000000.json"),
            b"not-json",
        )
        .unwrap();

        let loaded = cache.load_latest(None, 1_500).unwrap().unwrap();
        assert_eq!(loaded.document.fetched_at_unix_ms, 1_000);
    }

    #[test]
    fn cache_prunes_old_entries() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = ModelCatalogCache::new(dir.path()).with_max_entries(2);
        cache.store(&document(1_000, 5_000)).unwrap();
        cache.store(&document(2_000, 5_000)).unwrap();
        cache.store(&document(3_000, 5_000)).unwrap();

        let count = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| is_final_cache_path(path))
            .count();
        assert_eq!(count, 2);
    }
}
