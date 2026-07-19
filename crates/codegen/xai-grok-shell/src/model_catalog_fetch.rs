//! Bounded network fetch for provider and enterprise model catalogs.
//!
//! Catalog credentials are sent only to the configured endpoint. Redirects are
//! disabled, response bodies are size-limited, and errors never include response
//! bodies, URL query parameters, or credential values.

use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::header::{ACCEPT, AUTHORIZATION, ETAG, HeaderValue};
use serde::Serialize;
use url::Url;
use xai_grok_sampling_types::{ModelMetadata, ModelProtocol};

use crate::model_catalog_runtime::{
    CatalogParseOptions, ModelCatalogCache, ModelCatalogCacheDocument, ModelCatalogCacheError,
    parse_dttn_registry_catalog, parse_openai_compatible_catalog,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogEndpointKind {
    OpenAiCompatible,
    DttnRegistry,
}

/// Bearer credential with redacted `Debug` output.
#[derive(Clone)]
pub struct CatalogCredential(String);

impl CatalogCredential {
    pub fn bearer(value: impl Into<String>) -> Result<Self, CatalogFetchError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(CatalogFetchError::InvalidCredential);
        }
        HeaderValue::from_str(&format!("Bearer {value}"))
            .map_err(|_| CatalogFetchError::InvalidCredential)?;
        Ok(Self(value))
    }

    fn authorization_value(&self) -> Result<HeaderValue, CatalogFetchError> {
        HeaderValue::from_str(&format!("Bearer {}", self.0))
            .map_err(|_| CatalogFetchError::InvalidCredential)
    }
}

impl fmt::Debug for CatalogCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CatalogCredential([REDACTED])")
    }
}

#[derive(Clone)]
pub struct CatalogFetchRequest {
    pub endpoint: Url,
    pub kind: CatalogEndpointKind,
    pub credential: Option<CatalogCredential>,
    pub default_protocol: ModelProtocol,
    pub timeout: Duration,
    pub cache_ttl: Duration,
    /// Allows plain HTTP only for localhost or an IP loopback address.
    pub allow_insecure_localhost: bool,
}

impl fmt::Debug for CatalogFetchRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let endpoint =
            catalog_origin(&self.endpoint).unwrap_or_else(|_| "<invalid-url>".to_string());
        formatter
            .debug_struct("CatalogFetchRequest")
            .field("endpoint", &endpoint)
            .field("kind", &self.kind)
            .field("credential", &self.credential)
            .field("default_protocol", &self.default_protocol)
            .field("timeout", &self.timeout)
            .field("cache_ttl", &self.cache_ttl)
            .field("allow_insecure_localhost", &self.allow_insecure_localhost)
            .finish()
    }
}

impl CatalogFetchRequest {
    pub fn new(endpoint: Url, kind: CatalogEndpointKind, default_protocol: ModelProtocol) -> Self {
        Self {
            endpoint,
            kind,
            credential: None,
            default_protocol,
            timeout: DEFAULT_TIMEOUT,
            cache_ttl: Duration::from_secs(24 * 60 * 60),
            allow_insecure_localhost: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogFetchOutcome {
    pub origin: String,
    pub revision: Option<String>,
    pub model_count: usize,
    pub fetched_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub cache_path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogFetchError {
    #[error("model catalog endpoint must use HTTPS")]
    InsecureEndpoint,
    #[error("model catalog endpoint must not contain URL credentials")]
    UrlCredentialsForbidden,
    #[error("model catalog endpoint URL is missing a host")]
    MissingHost,
    #[error("catalog timeout must be greater than zero")]
    InvalidTimeout,
    #[error("catalog cache TTL must be between one millisecond and seven days")]
    InvalidTtl,
    #[error("catalog bearer credential is blank or invalid")]
    InvalidCredential,
    #[error("catalog HTTP client could not be built: {0}")]
    ClientBuild(reqwest::Error),
    #[error("catalog request failed: {0}")]
    Network(String),
    #[error("catalog endpoint returned HTTP {0}")]
    HttpStatus(u16),
    #[error("catalog response exceeded {MAX_RESPONSE_BYTES} bytes")]
    ResponseTooLarge,
    #[error("catalog response is not valid JSON: {0}")]
    InvalidJson(serde_json::Error),
    #[error("catalog payload is invalid: {0}")]
    InvalidCatalog(String),
    #[error("catalog endpoint returned no models")]
    EmptyCatalog,
    #[error("system clock is before the Unix epoch")]
    InvalidSystemClock,
    #[error(transparent)]
    Cache(#[from] ModelCatalogCacheError),
}

pub async fn fetch_and_cache_model_catalog(
    request: &CatalogFetchRequest,
    cache: &ModelCatalogCache,
) -> Result<CatalogFetchOutcome, CatalogFetchError> {
    validate_request(request)?;
    let origin = catalog_origin(&request.endpoint)?;
    let client = reqwest::Client::builder()
        .connect_timeout(request.timeout)
        .timeout(request.timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(CatalogFetchError::ClientBuild)?;

    let mut builder = client
        .get(request.endpoint.clone())
        .header(ACCEPT, "application/json");
    if let Some(credential) = &request.credential {
        builder = builder.header(AUTHORIZATION, credential.authorization_value()?);
    }

    let mut response = builder.send().await.map_err(network_error)?;
    if !response.status().is_success() {
        return Err(CatalogFetchError::HttpStatus(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(CatalogFetchError::ResponseTooLarge);
    }

    let revision = response
        .headers()
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(CatalogFetchError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    let payload: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(CatalogFetchError::InvalidJson)?;
    let options = CatalogParseOptions {
        origin: &origin,
        revision: revision.as_deref(),
        default_protocol: request.default_protocol,
    };
    let models = parse_payload(request.kind, &payload, &options)?;
    if models.is_empty() {
        return Err(CatalogFetchError::EmptyCatalog);
    }

    let fetched_at_unix_ms = unix_time_ms()?;
    let ttl_ms =
        u64::try_from(request.cache_ttl.as_millis()).map_err(|_| CatalogFetchError::InvalidTtl)?;
    let expires_at_unix_ms = fetched_at_unix_ms
        .checked_add(ttl_ms)
        .ok_or(CatalogFetchError::InvalidTtl)?;
    let document = ModelCatalogCacheDocument::new(
        origin.clone(),
        revision.clone(),
        fetched_at_unix_ms,
        expires_at_unix_ms,
        models,
    );
    let model_count = document.models.len();
    let cache_path = cache.store(&document)?;

    Ok(CatalogFetchOutcome {
        origin,
        revision,
        model_count,
        fetched_at_unix_ms,
        expires_at_unix_ms,
        cache_path,
    })
}

fn parse_payload(
    kind: CatalogEndpointKind,
    payload: &serde_json::Value,
    options: &CatalogParseOptions<'_>,
) -> Result<Vec<ModelMetadata>, CatalogFetchError> {
    let result = match kind {
        CatalogEndpointKind::OpenAiCompatible => parse_openai_compatible_catalog(payload, options),
        CatalogEndpointKind::DttnRegistry => parse_dttn_registry_catalog(payload, options),
    };
    result.map_err(|error| CatalogFetchError::InvalidCatalog(error.to_string()))
}

fn validate_request(request: &CatalogFetchRequest) -> Result<(), CatalogFetchError> {
    if request.endpoint.host_str().is_none() {
        return Err(CatalogFetchError::MissingHost);
    }
    if !request.endpoint.username().is_empty() || request.endpoint.password().is_some() {
        return Err(CatalogFetchError::UrlCredentialsForbidden);
    }
    match request.endpoint.scheme() {
        "https" => {}
        "http" if request.allow_insecure_localhost && endpoint_is_loopback(&request.endpoint) => {}
        _ => return Err(CatalogFetchError::InsecureEndpoint),
    }
    if request.timeout.is_zero() {
        return Err(CatalogFetchError::InvalidTimeout);
    }
    if request.cache_ttl.is_zero() || request.cache_ttl > MAX_TTL {
        return Err(CatalogFetchError::InvalidTtl);
    }
    Ok(())
}

fn endpoint_is_loopback(endpoint: &Url) -> bool {
    match endpoint.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

fn catalog_origin(endpoint: &Url) -> Result<String, CatalogFetchError> {
    if endpoint.host_str().is_none() {
        return Err(CatalogFetchError::MissingHost);
    }
    let mut origin = endpoint.clone();
    origin.set_query(None);
    origin.set_fragment(None);
    Ok(origin.to_string())
}

fn network_error(error: reqwest::Error) -> CatalogFetchError {
    CatalogFetchError::Network(error.without_url().to_string())
}

fn unix_time_ms() -> Result<u64, CatalogFetchError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CatalogFetchError::InvalidSystemClock)?
        .as_millis();
    u64::try_from(millis).map_err(|_| CatalogFetchError::InvalidSystemClock)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    #[test]
    fn credential_and_request_debug_are_redacted() {
        let credential = CatalogCredential::bearer("top-secret").unwrap();
        let rendered = format!("{credential:?}");
        assert!(!rendered.contains("top-secret"));
        assert!(rendered.contains("REDACTED"));

        let mut request = CatalogFetchRequest::new(
            Url::parse("https://provider.example/v1/models?api_key=query-secret").unwrap(),
            CatalogEndpointKind::OpenAiCompatible,
            ModelProtocol::ChatCompletions,
        );
        request.credential = Some(credential);
        let rendered = format!("{request:?}");
        assert!(!rendered.contains("top-secret"));
        assert!(!rendered.contains("query-secret"));
    }

    #[test]
    fn rejects_http_except_explicit_loopback() {
        let endpoint = Url::parse("http://provider.example/v1/models").unwrap();
        let request = CatalogFetchRequest::new(
            endpoint,
            CatalogEndpointKind::OpenAiCompatible,
            ModelProtocol::ChatCompletions,
        );
        assert!(matches!(
            validate_request(&request),
            Err(CatalogFetchError::InsecureEndpoint)
        ));

        let mut local = CatalogFetchRequest::new(
            Url::parse("http://127.0.0.1:1234/v1/models").unwrap(),
            CatalogEndpointKind::OpenAiCompatible,
            ModelProtocol::ChatCompletions,
        );
        local.allow_insecure_localhost = true;
        assert!(validate_request(&local).is_ok());
    }

    #[test]
    fn rejects_url_credentials_and_redacts_query_from_origin() {
        let mut request = CatalogFetchRequest::new(
            Url::parse("https://user:pass@provider.example/v1/models").unwrap(),
            CatalogEndpointKind::OpenAiCompatible,
            ModelProtocol::ChatCompletions,
        );
        assert!(matches!(
            validate_request(&request),
            Err(CatalogFetchError::UrlCredentialsForbidden)
        ));

        request.endpoint =
            Url::parse("https://provider.example/v1/models?api_key=secret#fragment").unwrap();
        assert_eq!(
            catalog_origin(&request.endpoint).unwrap(),
            "https://provider.example/v1/models"
        );
    }

    #[tokio::test]
    async fn network_error_does_not_expose_query_parameters() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = ModelCatalogCache::new(dir.path());
        let mut request = CatalogFetchRequest::new(
            Url::parse("http://127.0.0.1:0/v1/models?api_key=query-secret").unwrap(),
            CatalogEndpointKind::OpenAiCompatible,
            ModelProtocol::ChatCompletions,
        );
        request.allow_insecure_localhost = true;
        request.timeout = Duration::from_millis(100);
        let error = fetch_and_cache_model_catalog(&request, &cache)
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("query-secret"));
        assert!(!error.contains("api_key"));
    }

    #[tokio::test]
    async fn fetches_explicit_metadata_and_writes_cache_without_secret() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
            assert!(request.contains("authorization: bearer top-secret"));
            let body = r#"{"data":[{"id":"example","context_window":128000}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: revision-1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let dir = tempfile::TempDir::new().unwrap();
        let cache = ModelCatalogCache::new(dir.path());
        let mut request = CatalogFetchRequest::new(
            Url::parse(&format!("http://{address}/v1/models?tenant=secret")).unwrap(),
            CatalogEndpointKind::OpenAiCompatible,
            ModelProtocol::ChatCompletions,
        );
        request.allow_insecure_localhost = true;
        request.credential = Some(CatalogCredential::bearer("top-secret").unwrap());
        let outcome = fetch_and_cache_model_catalog(&request, &cache)
            .await
            .unwrap();
        server.join().unwrap();

        assert_eq!(outcome.model_count, 1);
        assert_eq!(outcome.revision.as_deref(), Some("revision-1"));
        assert!(!outcome.origin.contains("tenant=secret"));
        let cache_bytes = fs::read(outcome.cache_path).unwrap();
        let cache_text = String::from_utf8(cache_bytes).unwrap();
        assert!(!cache_text.contains("top-secret"));
        assert!(!cache_text.contains("tenant=secret"));
    }

    #[tokio::test]
    async fn does_not_follow_redirects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: https://other.example/models\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        });

        let dir = tempfile::TempDir::new().unwrap();
        let cache = ModelCatalogCache::new(dir.path());
        let mut request = CatalogFetchRequest::new(
            Url::parse(&format!("http://{address}/v1/models")).unwrap(),
            CatalogEndpointKind::OpenAiCompatible,
            ModelProtocol::ChatCompletions,
        );
        request.allow_insecure_localhost = true;
        let error = fetch_and_cache_model_catalog(&request, &cache)
            .await
            .unwrap_err();
        server.join().unwrap();
        assert!(matches!(error, CatalogFetchError::HttpStatus(302)));
    }
}
