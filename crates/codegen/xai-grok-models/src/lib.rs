//! Default model IDs loaded from `default_models.json` at runtime.
//! Edit that JSON file to change the distribution defaults.
//!
//! At runtime each model is resolved via:
//!   CLI flag > environment variable > config.toml > remote settings > these defaults

use std::sync::LazyLock;

/// The raw JSON, embedded at compile time and consumed by the runtime config layer.
pub const DEFAULT_MODELS_JSON: &str = include_str!("../default_models.json");

#[derive(serde::Deserialize)]
struct DefaultModels {
    default: String,
    /// Falls back to `default` if not specified in JSON.
    web_search: Option<String>,
    /// Falls back to `default` if not specified in JSON.
    image_description: Option<String>,
    /// Falls back to `default` if not specified in JSON.
    session_summary: Option<String>,
    models: Vec<DefaultModelEntry>,
}

#[derive(serde::Deserialize)]
struct DefaultModelEntry {
    model: String,
}

static DEFAULTS: LazyLock<DefaultModels> = LazyLock::new(|| {
    let defaults: DefaultModels = serde_json::from_str(DEFAULT_MODELS_JSON)
        .expect("default_models.json: invalid JSON or missing 'default' field");

    // Baked-in JSON — a mismatch here is a developer error, not a runtime condition.
    let model_ids: Vec<&str> = defaults.models.iter().map(|m| m.model.as_str()).collect();
    assert!(
        model_ids.contains(&defaults.default.as_str()),
        "default_models.json: 'default' is '{}' but 'models' array only has {model_ids:?}",
        defaults.default,
    );

    defaults
});

/// Primary model for coding tasks and general fallback.
pub fn default_model() -> &'static str {
    &DEFAULTS.default
}

/// Model for web-search synthesis. Falls back to the primary model.
pub fn default_web_search_model() -> &'static str {
    DEFAULTS.web_search.as_deref().unwrap_or(&DEFAULTS.default)
}

/// Model for image description. Falls back to the primary model.
pub fn default_image_description_model() -> &'static str {
    DEFAULTS
        .image_description
        .as_deref()
        .unwrap_or(&DEFAULTS.default)
}

/// Model for session-title generation. Falls back to the primary model.
pub fn default_session_summary_model() -> &'static str {
    DEFAULTS
        .session_summary
        .as_deref()
        .unwrap_or(&DEFAULTS.default)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISTRIBUTION_DEFAULT_MODEL: &str = "agnes-2.0-flash";

    #[test]
    fn distribution_defaults_resolve_to_agnes() {
        assert_eq!(default_model(), DISTRIBUTION_DEFAULT_MODEL);
        assert_eq!(default_web_search_model(), DISTRIBUTION_DEFAULT_MODEL);
        assert_eq!(default_image_description_model(), DISTRIBUTION_DEFAULT_MODEL);
        assert_eq!(default_session_summary_model(), DISTRIBUTION_DEFAULT_MODEL);
    }

    #[test]
    fn agnes_catalog_matches_documented_public_api_contract() {
        let catalog: serde_json::Value = serde_json::from_str(DEFAULT_MODELS_JSON).unwrap();
        let model = &catalog["models"][0];

        assert_eq!(model["model"], "agnes-2.0-flash");
        assert_eq!(model["api_backend"], "chat_completions");
        assert_eq!(model["provider_extensions"], "standard");
        assert_eq!(model["context_window"], 262_144);
        assert_eq!(model["max_completion_tokens"], 65_536);
    }

    #[test]
    fn distribution_model_catalog_has_no_legacy_vendor_branding() {
        assert!(!DEFAULT_MODELS_JSON.to_ascii_lowercase().contains("grok"));
    }
}
