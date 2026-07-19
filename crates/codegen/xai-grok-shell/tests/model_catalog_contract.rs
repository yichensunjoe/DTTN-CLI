use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use serde_json::json;
use url::Url;
use xai_grok_sampling_types::{MetadataSource, ModelMetadata, ModelProtocol, Sourced};
use xai_grok_shell::model_catalog_fetch::{
    CatalogCredential, CatalogEndpointKind, CatalogFetchError, CatalogFetchRequest,
    fetch_and_cache_model_catalog,
};
use xai_grok_shell::model_catalog_runtime::{
    CatalogFreshness, CatalogParseError, CatalogParseOptions, ModelCatalogCache,
    ModelCatalogCacheDocument, parse_dttn_registry_catalog, parse_openai_compatible_catalog,
};

fn parse_options<'a>() -> CatalogParseOptions<'a> {
    CatalogParseOptions {
        origin: "https://provider.example/v1/models",
        revision: Some("etag-123"),
        default_protocol: ModelProtocol::ChatCompletions,
    }
}

#[test]
fn openai_extensions_are_explicit_and_source_tracked() {
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

    let model = parse_openai_compatible_catalog(&payload, &parse_options())
        .unwrap()
        .remove(0);
    assert_eq!(model.model_id, "example-model");
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
fn identity_only_models_remain_unknown() {
    let payload = json!({"data": [{"id": "identity-only"}]});
    let model = parse_openai_compatible_catalog(&payload, &parse_options())
        .unwrap()
        .remove(0);
    assert!(model.context_window.is_none());
    assert!(model.max_output_tokens.is_none());
    assert!(model.pricing.input_per_million_microunits.is_none());
    assert!(model.capabilities.tool_calling.is_none());
}

#[test]
fn enterprise_registry_requires_schema_and_rejects_duplicates() {
    let payload = json!({
        "schema_version": 1,
        "models": [{"id": "company/model", "context_window": 128000}]
    });
    let models = parse_dttn_registry_catalog(&payload, &parse_options()).unwrap();
    assert_eq!(
        models[0].context_window.as_ref().unwrap().source,
        MetadataSource::EnterpriseRegistry
    );

    let unsupported = json!({"schema_version": 2, "models": []});
    assert!(matches!(
        parse_dttn_registry_catalog(&unsupported, &parse_options()),
        Err(CatalogParseError::UnsupportedSchema(2))
    ));

    let duplicates = json!({"data": [{"id": "same"}, {"id": "same"}]});
    assert!(matches!(
        parse_openai_compatible_catalog(&duplicates, &parse_options()),
        Err(CatalogParseError::DuplicateModelId(id)) if id == "same"
    ));
}

fn cache_document(
    origin: &str,
    model_id: &str,
    fetched_at: u64,
    expires_at: u64,
) -> ModelCatalogCacheDocument {
    ModelCatalogCacheDocument::new(
        origin,
        Some("etag-123".to_string()),
        fetched_at,
        expires_at,
        vec![ModelMetadata {
            model_id: model_id.to_string(),
            context_window: Some(Sourced::new(262_144, MetadataSource::ProviderApi)),
            ..Default::default()
        }],
    )
}

#[test]
fn append_only_cache_round_trip_staleness_fallback_and_pruning() {
    let dir = tempfile::TempDir::new().unwrap();
    let cache = ModelCatalogCache::new(dir.path()).with_max_entries(2);
    let origin = "https://provider.example/v1/models";
    cache
        .store(&cache_document(origin, "model-a", 1_000, 5_000))
        .unwrap();

    let fresh = cache.load_latest(Some(origin), 1_500).unwrap().unwrap();
    assert_eq!(fresh.freshness, CatalogFreshness::Fresh);
    assert_eq!(fresh.document.models[0].model_id, "model-a");
    let stale = cache.load_latest(Some(origin), 5_000).unwrap().unwrap();
    assert_eq!(stale.freshness, CatalogFreshness::Stale);
    assert!(
        cache
            .load_latest(Some("https://other.example/v1/models"), 1_500)
            .unwrap()
            .is_none()
    );

    fs::write(
        dir.path()
            .join("catalog-00000000000000002000-0000000001-00000000000000000000.json"),
        b"not-json",
    )
    .unwrap();
    let fallback = cache.load_latest(None, 1_500).unwrap().unwrap();
    assert_eq!(fallback.document.fetched_at_unix_ms, 1_000);

    cache
        .store(&cache_document(origin, "model-b", 3_000, 6_000))
        .unwrap();
    cache
        .store(&cache_document(origin, "model-c", 4_000, 7_000))
        .unwrap();
    let count = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("catalog-") && name.ends_with(".json"))
        .count();
    assert_eq!(count, 2);
}

#[test]
fn request_debug_redacts_credentials_and_query() {
    let credential = CatalogCredential::bearer("top-secret").unwrap();
    let rendered = format!("{credential:?}");
    assert!(!rendered.contains("top-secret"));

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

#[tokio::test]
async fn unsafe_endpoint_and_network_errors_are_safely_reported() {
    let dir = tempfile::TempDir::new().unwrap();
    let cache = ModelCatalogCache::new(dir.path());

    let request = CatalogFetchRequest::new(
        Url::parse("http://provider.example/v1/models").unwrap(),
        CatalogEndpointKind::OpenAiCompatible,
        ModelProtocol::ChatCompletions,
    );
    assert!(matches!(
        fetch_and_cache_model_catalog(&request, &cache).await,
        Err(CatalogFetchError::InsecureEndpoint)
    ));

    let mut local = CatalogFetchRequest::new(
        Url::parse("http://127.0.0.1:0/v1/models?api_key=query-secret").unwrap(),
        CatalogEndpointKind::OpenAiCompatible,
        ModelProtocol::ChatCompletions,
    );
    local.allow_insecure_localhost = true;
    local.timeout = Duration::from_millis(100);
    let error = fetch_and_cache_model_catalog(&local, &cache)
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("query-secret"));
    assert!(!error.contains("api_key"));
}

#[tokio::test]
async fn fetch_sends_bearer_once_and_cache_contains_no_secret() {
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
    let cache_text = fs::read_to_string(outcome.cache_path).unwrap();
    assert!(!cache_text.contains("top-secret"));
    assert!(!cache_text.contains("tenant=secret"));
}

#[tokio::test]
async fn redirect_is_not_followed() {
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
