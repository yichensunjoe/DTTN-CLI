//! Doctor-only model catalog refresh orchestration.
//!
//! This layer converts an explicit diagnostic request into the bounded catalog
//! fetcher. It never runs during ordinary startup. Inference credentials are
//! reused only for same-origin metadata endpoints; cross-origin registries need
//! a dedicated credential or receive no credential.

use std::fmt;
use std::time::Duration;

use serde::Serialize;
use url::Url;
use xai_grok_sampling_types::ModelProtocol;

use crate::model_catalog_fetch::{
    CatalogCredential, CatalogEndpointKind, CatalogFetchError, CatalogFetchOutcome,
    CatalogFetchRequest, fetch_and_cache_model_catalog,
};
use crate::model_catalog_runtime::ModelCatalogCache;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorCatalogCredentialSource {
    None,
    SameOriginInference,
    Dedicated,
}

#[derive(Clone)]
pub struct DoctorCatalogRefreshOptions {
    pub provider_base_url: Url,
    pub endpoint_override: Option<Url>,
    pub kind: CatalogEndpointKind,
    pub default_protocol: ModelProtocol,
    pub inference_credential: Option<CatalogCredential>,
    pub dedicated_credential: Option<CatalogCredential>,
    pub timeout: Duration,
    pub cache_ttl: Duration,
    pub allow_insecure_localhost: bool,
}

impl DoctorCatalogRefreshOptions {
    pub fn new(
        provider_base_url: Url,
        kind: CatalogEndpointKind,
        default_protocol: ModelProtocol,
    ) -> Self {
        Self {
            provider_base_url,
            endpoint_override: None,
            kind,
            default_protocol,
            inference_credential: None,
            dedicated_credential: None,
            timeout: Duration::from_secs(5),
            cache_ttl: Duration::from_secs(24 * 60 * 60),
            allow_insecure_localhost: false,
        }
    }
}

impl fmt::Debug for DoctorCatalogRefreshOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DoctorCatalogRefreshOptions")
            .field("provider_base_url", &redact_url(&self.provider_base_url))
            .field("endpoint_override", &self.endpoint_override.as_ref().map(redact_url))
            .field("kind", &self.kind)
            .field("default_protocol", &self.default_protocol)
            .field("inference_credential", &self.inference_credential)
            .field("dedicated_credential", &self.dedicated_credential)
            .field("timeout", &self.timeout)
            .field("cache_ttl", &self.cache_ttl)
            .field("allow_insecure_localhost", &self.allow_insecure_localhost)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorCatalogRefreshOutcome {
    pub endpoint: String,
    pub credential_source: DoctorCatalogCredentialSource,
    pub catalog: CatalogFetchOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum DoctorCatalogRefreshError {
    #[error("provider base URL cannot be used to derive a model catalog endpoint")]
    InvalidProviderBaseUrl,
    #[error(transparent)]
    Fetch(#[from] CatalogFetchError),
}

pub async fn refresh_model_catalog_for_doctor(
    options: &DoctorCatalogRefreshOptions,
    cache: &ModelCatalogCache,
) -> Result<DoctorCatalogRefreshOutcome, DoctorCatalogRefreshError> {
    let endpoint = match &options.endpoint_override {
        Some(endpoint) => endpoint.clone(),
        None => derive_models_endpoint(&options.provider_base_url)?,
    };
    let (credential, credential_source) = select_credential(options, &endpoint);

    let mut request = CatalogFetchRequest::new(
        endpoint.clone(),
        options.kind,
        options.default_protocol,
    );
    request.credential = credential;
    request.timeout = options.timeout;
    request.cache_ttl = options.cache_ttl;
    request.allow_insecure_localhost = options.allow_insecure_localhost;

    let catalog = fetch_and_cache_model_catalog(&request, cache).await?;
    Ok(DoctorCatalogRefreshOutcome {
        endpoint: redact_url(&endpoint),
        credential_source,
        catalog,
    })
}

pub fn derive_models_endpoint(base_url: &Url) -> Result<Url, DoctorCatalogRefreshError> {
    if base_url.cannot_be_a_base() || base_url.host_str().is_none() {
        return Err(DoctorCatalogRefreshError::InvalidProviderBaseUrl);
    }

    let mut endpoint = base_url.clone();
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    let _ = endpoint.set_username("");
    let _ = endpoint.set_password(None);

    let mut path = endpoint.path().trim_end_matches('/').to_string();
    for suffix in ["/chat/completions", "/responses", "/messages"] {
        if let Some(prefix) = path.strip_suffix(suffix) {
            path = prefix.to_string();
            break;
        }
    }
    if !path.ends_with("/models") {
        path.push_str("/models");
    }
    endpoint.set_path(&path);
    Ok(endpoint)
}

fn select_credential(
    options: &DoctorCatalogRefreshOptions,
    endpoint: &Url,
) -> (Option<CatalogCredential>, DoctorCatalogCredentialSource) {
    if let Some(credential) = options.dedicated_credential.clone() {
        return (Some(credential), DoctorCatalogCredentialSource::Dedicated);
    }
    if same_origin(endpoint, &options.provider_base_url)
        && let Some(credential) = options.inference_credential.clone()
    {
        return (
            Some(credential),
            DoctorCatalogCredentialSource::SameOriginInference,
        );
    }
    (None, DoctorCatalogCredentialSource::None)
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str().map(str::to_ascii_lowercase)
            == right.host_str().map(str::to_ascii_lowercase)
        && left.port_or_known_default() == right.port_or_known_default()
}

fn redact_url(value: &Url) -> String {
    let mut url = value.clone();
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string().trim_end_matches('/').to_string()
}
