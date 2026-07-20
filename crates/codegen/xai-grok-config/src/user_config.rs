//! Focused read/write access to the user-owned DTTN config layer.
//!
//! This module never reads managed configuration and never starts network or
//! model runtime work. Updates preserve unrelated TOML formatting and comments.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use thiserror::Error;
use toml_edit::{DocumentMut, Item, Table, value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomModelApiBackend {
    ChatCompletions,
    Responses,
    Messages,
}

impl CustomModelApiBackend {
    pub fn as_config_value(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Responses => "responses",
            Self::Messages => "messages",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomModelAuthScheme {
    Bearer,
    XApiKey,
}

impl CustomModelAuthScheme {
    pub fn as_config_value(self) -> &'static str {
        match self {
            Self::Bearer => "bearer",
            Self::XApiKey => "x_api_key",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomModelConfig {
    pub provider_id: String,
    pub model_id: String,
    pub display_name: Option<String>,
    pub base_url: String,
    pub api_key_env: Option<String>,
    pub api_backend: CustomModelApiBackend,
    pub auth_scheme: CustomModelAuthScheme,
    pub context_window: u64,
    pub max_completion_tokens: Option<u32>,
    pub set_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomModelWriteResult {
    pub path: PathBuf,
    pub model_ref: String,
    pub default_changed: bool,
}

#[derive(Debug, Error)]
pub enum UserConfigError {
    #[error(
        "invalid model id: model must not be empty or contain whitespace or control characters"
    )]
    InvalidModelId,
    #[error("invalid provider id: use lowercase letters, numbers, '.', '_' or '-'")]
    InvalidProviderId,
    #[error("invalid base URL: use HTTPS, or HTTP only for localhost/loopback endpoints")]
    InvalidBaseUrl,
    #[error("invalid API key environment variable name")]
    InvalidApiKeyEnv,
    #[error("remote custom providers require an API key environment variable")]
    MissingApiKeyEnv,
    #[error("context window must be greater than zero and fit in a TOML integer")]
    InvalidContextWindow,
    #[error("max completion tokens must be greater than zero")]
    InvalidMaxCompletionTokens,
    #[error("failed to read DTTN config at {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse DTTN config at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml_edit::TomlError,
    },
    #[error("[models] in {path} is not a TOML table")]
    ModelsNotTable { path: PathBuf },
    #[error("[model] in {path} is not a TOML table")]
    ModelCatalogNotTable { path: PathBuf },
    #[error("model entry {model_ref} in {path} is not a TOML table")]
    ModelEntryNotTable { path: PathBuf, model_ref: String },
    #[error("failed to write DTTN config at {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn user_config_path() -> PathBuf {
    crate::dttn_home().join("config.toml")
}

pub fn user_default_model() -> Result<Option<String>, UserConfigError> {
    user_default_model_at(&user_config_path())
}

pub fn set_user_default_model(model: &str) -> Result<PathBuf, UserConfigError> {
    let path = user_config_path();
    set_user_default_model_at(&path, model)?;
    Ok(path)
}

pub fn reset_user_default_model() -> Result<bool, UserConfigError> {
    reset_user_default_model_at(&user_config_path())
}

pub fn set_custom_model(
    config: &CustomModelConfig,
) -> Result<CustomModelWriteResult, UserConfigError> {
    let path = user_config_path();
    let model_ref = set_custom_model_at(&path, config)?;
    Ok(CustomModelWriteResult {
        path,
        model_ref,
        default_changed: config.set_default,
    })
}

fn validate_model_id(model: &str) -> Result<&str, UserConfigError> {
    let model = model.trim();
    if model.is_empty()
        || model.starts_with('/')
        || model.ends_with('/')
        || model
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(UserConfigError::InvalidModelId);
    }
    Ok(model)
}

fn validate_provider_id(provider: &str) -> Result<&str, UserConfigError> {
    let provider = provider.trim();
    let mut chars = provider.chars();
    let Some(first) = chars.next() else {
        return Err(UserConfigError::InvalidProviderId);
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(UserConfigError::InvalidProviderId);
    }
    if !chars
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-'))
    {
        return Err(UserConfigError::InvalidProviderId);
    }
    Ok(provider)
}

fn validate_api_key_env(name: &str) -> Result<&str, UserConfigError> {
    let name = name.trim();
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(UserConfigError::InvalidApiKeyEnv);
    };
    if !first.is_ascii_uppercase() && first != '_' {
        return Err(UserConfigError::InvalidApiKeyEnv);
    }
    if !chars.all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_') {
        return Err(UserConfigError::InvalidApiKeyEnv);
    }
    Ok(name)
}

fn authority_host(authority: &str) -> Option<&str> {
    if let Some(bracketed) = authority.strip_prefix('[') {
        let end = bracketed.find(']')?;
        let host = &bracketed[..end];
        let suffix = &bracketed[end + 1..];
        let port_is_valid = suffix.is_empty()
            || suffix
                .strip_prefix(':')
                .is_some_and(|port| !port.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()));
        return port_is_valid.then_some(host);
    }

    match authority.split_once(':') {
        Some((host, port))
            if !host.is_empty()
                && !port.is_empty()
                && !host.contains(':')
                && port.chars().all(|ch| ch.is_ascii_digit()) =>
        {
            Some(host)
        }
        Some(_) => None,
        None if !authority.is_empty() => Some(authority),
        None => None,
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn normalize_base_url(raw: &str) -> Result<(String, bool), UserConfigError> {
    let url = raw.trim().trim_end_matches('/');
    if url.is_empty() || url.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return Err(UserConfigError::InvalidBaseUrl);
    }
    let (rest, is_https) = if let Some(rest) = url.strip_prefix("https://") {
        (rest, true)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (rest, false)
    } else {
        return Err(UserConfigError::InvalidBaseUrl);
    };
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return Err(UserConfigError::InvalidBaseUrl);
    }
    let host = authority_host(authority).ok_or(UserConfigError::InvalidBaseUrl)?;
    let is_loopback = is_loopback_host(host);
    if !is_https && !is_loopback {
        return Err(UserConfigError::InvalidBaseUrl);
    }
    Ok((url.to_owned(), is_loopback))
}

fn load_document(path: &Path) -> Result<DocumentMut, UserConfigError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DocumentMut::new());
        }
        Err(source) => {
            return Err(UserConfigError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    DocumentMut::from_str(&raw).map_err(|source| UserConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

fn user_default_model_at(path: &Path) -> Result<Option<String>, UserConfigError> {
    let document = load_document(path)?;
    Ok(document
        .get("models")
        .and_then(Item::as_table_like)
        .and_then(|models| models.get("default"))
        .and_then(Item::as_value)
        .and_then(|value| value.as_str())
        .map(str::to_owned))
}

fn set_user_default_model_at(path: &Path, model: &str) -> Result<(), UserConfigError> {
    let model = validate_model_id(model)?;
    let mut document = load_document(path)?;
    set_default_model_in_document(path, &mut document, model)?;
    write_document(path, &document)
}

fn set_default_model_in_document(
    path: &Path,
    document: &mut DocumentMut,
    model: &str,
) -> Result<(), UserConfigError> {
    if document.get("models").is_none() {
        document["models"] = Item::Table(Table::new());
    }
    let Some(models) = document.get_mut("models").and_then(Item::as_table_like_mut) else {
        return Err(UserConfigError::ModelsNotTable {
            path: path.to_path_buf(),
        });
    };
    models.insert("default", value(model));
    Ok(())
}

fn set_custom_model_at(path: &Path, config: &CustomModelConfig) -> Result<String, UserConfigError> {
    let provider_id = validate_provider_id(&config.provider_id)?;
    let model_id = validate_model_id(&config.model_id)?;
    let (base_url, is_loopback) = normalize_base_url(&config.base_url)?;
    let api_key_env = config
        .api_key_env
        .as_deref()
        .map(validate_api_key_env)
        .transpose()?;
    if api_key_env.is_none() && !is_loopback {
        return Err(UserConfigError::MissingApiKeyEnv);
    }
    if config.context_window == 0 || i64::try_from(config.context_window).is_err() {
        return Err(UserConfigError::InvalidContextWindow);
    }
    if config.max_completion_tokens == Some(0) {
        return Err(UserConfigError::InvalidMaxCompletionTokens);
    }

    let model_ref = format!("{provider_id}/{model_id}");
    let mut document = load_document(path)?;
    if document.get("model").is_none() {
        document["model"] = Item::Table(Table::new());
    }
    let Some(catalog) = document.get_mut("model").and_then(Item::as_table_like_mut) else {
        return Err(UserConfigError::ModelCatalogNotTable {
            path: path.to_path_buf(),
        });
    };
    if catalog.get(&model_ref).is_none() {
        catalog.insert(&model_ref, Item::Table(Table::new()));
    }
    let Some(entry) = catalog
        .get_mut(&model_ref)
        .and_then(Item::as_table_like_mut)
    else {
        return Err(UserConfigError::ModelEntryNotTable {
            path: path.to_path_buf(),
            model_ref,
        });
    };

    entry.insert("model", value(model_id));
    entry.insert("base_url", value(base_url));
    if let Some(api_key_env) = api_key_env {
        entry.insert("env_key", value(api_key_env));
    } else {
        entry.remove("env_key");
    }
    entry.insert("api_backend", value(config.api_backend.as_config_value()));
    entry.insert("auth_scheme", value(config.auth_scheme.as_config_value()));
    entry.insert("provider_extensions", value("standard"));
    entry.insert("context_window", value(config.context_window as i64));
    entry.insert("agent_type", value("dttn-code-agent"));
    entry.insert("supported_in_api", value(true));
    if let Some(name) = config
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        entry.insert("name", value(name));
    } else {
        entry.remove("name");
    }
    if let Some(max_tokens) = config.max_completion_tokens {
        entry.insert("max_completion_tokens", value(i64::from(max_tokens)));
    } else {
        entry.remove("max_completion_tokens");
    }

    if config.set_default {
        set_default_model_in_document(path, &mut document, &model_ref)?;
    }
    write_document(path, &document)?;
    Ok(model_ref)
}

fn reset_user_default_model_at(path: &Path) -> Result<bool, UserConfigError> {
    if !path.exists() {
        return Ok(false);
    }
    let mut document = load_document(path)?;
    let removed = match document.get_mut("models") {
        Some(item) => item
            .as_table_like_mut()
            .ok_or_else(|| UserConfigError::ModelsNotTable {
                path: path.to_path_buf(),
            })?
            .remove("default")
            .is_some(),
        None => false,
    };
    if removed {
        write_document(path, &document)?;
    }
    Ok(removed)
}

fn write_document(path: &Path, document: &DocumentMut) -> Result<(), UserConfigError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| UserConfigError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    }
    super::fs_atomic::write_atomically(path, &document.to_string(), Some(0o600)).map_err(|source| {
        UserConfigError::Write {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_model_preserves_comments_and_unrelated_tables() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(
            &path,
            "# keep this comment\n[ui]\nscreen_mode = \"minimal\"\n",
        )
        .unwrap();
        set_user_default_model_at(&path, "anthropic/claude-sonnet").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# keep this comment"));
        assert!(raw.contains("screen_mode = \"minimal\""));
        assert!(raw.contains("default = \"anthropic/claude-sonnet\""));
    }

    #[test]
    fn custom_model_uses_existing_runtime_schema_and_does_not_switch_implicitly() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "# retained\n[models]\ndefault = \"old/model\"\n").unwrap();
        let config = CustomModelConfig {
            provider_id: "acme".to_owned(),
            model_id: "code-v1".to_owned(),
            display_name: Some("Acme Code".to_owned()),
            base_url: "https://models.acme.test/v1/".to_owned(),
            api_key_env: Some("ACME_API_KEY".to_owned()),
            api_backend: CustomModelApiBackend::ChatCompletions,
            auth_scheme: CustomModelAuthScheme::Bearer,
            context_window: 131_072,
            max_completion_tokens: Some(8192),
            set_default: false,
        };
        assert_eq!(set_custom_model_at(&path, &config).unwrap(), "acme/code-v1");
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# retained"));
        assert!(raw.contains("default = \"old/model\""));
        assert!(raw.contains("[model.\"acme/code-v1\"]"));
        assert!(raw.contains("model = \"code-v1\""));
        assert!(raw.contains("base_url = \"https://models.acme.test/v1\""));
        assert!(raw.contains("env_key = \"ACME_API_KEY\""));
        assert!(raw.contains("api_backend = \"chat_completions\""));
        assert!(raw.contains("auth_scheme = \"bearer\""));
        assert!(raw.contains("provider_extensions = \"standard\""));
        assert!(raw.contains("context_window = 131072"));
    }

    #[test]
    fn custom_model_switches_default_only_when_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let config = CustomModelConfig {
            provider_id: "local".to_owned(),
            model_id: "model:latest".to_owned(),
            display_name: None,
            base_url: "http://127.0.0.1:1234/v1".to_owned(),
            api_key_env: None,
            api_backend: CustomModelApiBackend::Responses,
            auth_scheme: CustomModelAuthScheme::Bearer,
            context_window: 65_536,
            max_completion_tokens: None,
            set_default: true,
        };
        set_custom_model_at(&path, &config).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("default = \"local/model:latest\""));
        assert!(raw.contains("api_backend = \"responses\""));
        assert!(!raw.contains("env_key ="));
    }

    #[test]
    fn https_loopback_endpoint_may_omit_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let config = CustomModelConfig {
            provider_id: "local".to_owned(),
            model_id: "secure-local".to_owned(),
            display_name: None,
            base_url: "https://127.0.0.2:8443/v1".to_owned(),
            api_key_env: None,
            api_backend: CustomModelApiBackend::ChatCompletions,
            auth_scheme: CustomModelAuthScheme::Bearer,
            context_window: 32_768,
            max_completion_tokens: None,
            set_default: false,
        };
        set_custom_model_at(&path, &config).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("base_url = \"https://127.0.0.2:8443/v1\""));
        assert!(!raw.contains("env_key ="));
    }

    #[test]
    fn reregistering_custom_model_clears_omitted_optional_fields() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let mut config = CustomModelConfig {
            provider_id: "acme".to_owned(),
            model_id: "code-v1".to_owned(),
            display_name: Some("Old Name".to_owned()),
            base_url: "https://models.acme.test/v1".to_owned(),
            api_key_env: Some("ACME_API_KEY".to_owned()),
            api_backend: CustomModelApiBackend::ChatCompletions,
            auth_scheme: CustomModelAuthScheme::Bearer,
            context_window: 131_072,
            max_completion_tokens: Some(8192),
            set_default: false,
        };
        set_custom_model_at(&path, &config).unwrap();
        config.display_name = None;
        config.max_completion_tokens = None;
        set_custom_model_at(&path, &config).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("name = \"Old Name\""));
        assert!(!raw.contains("max_completion_tokens ="));
    }

    #[test]
    fn credentials_embedded_in_base_url_are_rejected_without_writing() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let config = CustomModelConfig {
            provider_id: "unsafe".to_owned(),
            model_id: "model".to_owned(),
            display_name: None,
            base_url: "https://secret@models.example.test/v1".to_owned(),
            api_key_env: Some("UNSAFE_API_KEY".to_owned()),
            api_backend: CustomModelApiBackend::ChatCompletions,
            auth_scheme: CustomModelAuthScheme::Bearer,
            context_window: 4096,
            max_completion_tokens: None,
            set_default: false,
        };
        assert!(matches!(
            set_custom_model_at(&path, &config),
            Err(UserConfigError::InvalidBaseUrl)
        ));
        assert!(!path.exists());
    }

    #[test]
    fn remote_custom_endpoint_requires_an_api_key_environment_variable() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let config = CustomModelConfig {
            provider_id: "remote".to_owned(),
            model_id: "model".to_owned(),
            display_name: None,
            base_url: "https://models.example.test/v1".to_owned(),
            api_key_env: None,
            api_backend: CustomModelApiBackend::ChatCompletions,
            auth_scheme: CustomModelAuthScheme::Bearer,
            context_window: 4096,
            max_completion_tokens: None,
            set_default: false,
        };
        assert!(matches!(
            set_custom_model_at(&path, &config),
            Err(UserConfigError::MissingApiKeyEnv)
        ));
        assert!(!path.exists());
    }

    #[test]
    fn remote_plaintext_custom_endpoint_is_rejected_without_writing() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let config = CustomModelConfig {
            provider_id: "unsafe".to_owned(),
            model_id: "model".to_owned(),
            display_name: None,
            base_url: "http://models.example.test/v1".to_owned(),
            api_key_env: Some("UNSAFE_API_KEY".to_owned()),
            api_backend: CustomModelApiBackend::ChatCompletions,
            auth_scheme: CustomModelAuthScheme::Bearer,
            context_window: 4096,
            max_completion_tokens: None,
            set_default: false,
        };
        assert!(matches!(
            set_custom_model_at(&path, &config),
            Err(UserConfigError::InvalidBaseUrl)
        ));
        assert!(!path.exists());
    }

    #[test]
    fn reset_removes_only_the_default_model() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(
            &path,
            "[models]\ndefault = \"old\"\nweb_search = \"search\"\n",
        )
        .unwrap();
        assert!(reset_user_default_model_at(&path).unwrap());
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("default ="));
        assert!(raw.contains("web_search = \"search\""));
    }

    #[test]
    fn invalid_existing_toml_is_never_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "[models\n").unwrap();
        assert!(matches!(
            set_user_default_model_at(&path, "model"),
            Err(UserConfigError::Parse { .. })
        ));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "[models\n");
    }
}
