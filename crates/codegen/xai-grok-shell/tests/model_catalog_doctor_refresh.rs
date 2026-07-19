use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use url::Url;
use xai_grok_sampling_types::ModelProtocol;
use xai_grok_shell::model_catalog_doctor_refresh::{
    DoctorCatalogCredentialSource, DoctorCatalogRefreshOptions, derive_models_endpoint,
    refresh_model_catalog_for_doctor,
};
use xai_grok_shell::model_catalog_fetch::{CatalogCredential, CatalogEndpointKind};
use xai_grok_shell::model_catalog_runtime::ModelCatalogCache;

fn spawn_catalog_server(
    expect_authorization: Option<&'static str>,
) -> (std::net::SocketAddr, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        let read = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
        match expect_authorization {
            Some(value) => assert!(request.contains(value)),
            None => assert!(!request.contains("authorization:")),
        }
        let body = r#"{"data":[{"id":"example","context_window":128000}]}"#;
        write!(
            stream,
            concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: application/json\r\n",
                "ETag: revision-1\r\n",
                "Content-Length: {}\r\n",
                "Connection: close\r\n\r\n",
                "{}"
            ),
            body.len(),
            body
        )
        .unwrap();
    });
    (address, server)
}

#[test]
fn derives_models_endpoint_without_query_or_protocol_suffix() {
    assert_eq!(
        derive_models_endpoint(&Url::parse("https://provider.example/v1").unwrap())
            .unwrap()
            .as_str(),
        "https://provider.example/v1/models"
    );
    assert_eq!(
        derive_models_endpoint(
            &Url::parse("https://provider.example/v1/chat/completions?api_key=secret").unwrap()
        )
        .unwrap()
        .as_str(),
        "https://provider.example/v1/models"
    );
}

#[test]
fn options_debug_redacts_credentials_and_queries() {
    let mut options = DoctorCatalogRefreshOptions::new(
        Url::parse("https://provider.example/v1?provider_secret=1").unwrap(),
        CatalogEndpointKind::OpenAiCompatible,
        ModelProtocol::ChatCompletions,
    );
    options.endpoint_override = Some(
        Url::parse("https://registry.example/models?registry_secret=1").unwrap(),
    );
    options.inference_credential = Some(CatalogCredential::bearer("top-secret").unwrap());
    let rendered = format!("{options:?}");
    assert!(!rendered.contains("top-secret"));
    assert!(!rendered.contains("provider_secret"));
    assert!(!rendered.contains("registry_secret"));
}

#[tokio::test]
async fn same_origin_refresh_reuses_inference_bearer() {
    let (address, server) = spawn_catalog_server(Some("authorization: bearer inference-secret"));
    let base = Url::parse(&format!("http://{address}/v1")).unwrap();
    let mut options = DoctorCatalogRefreshOptions::new(
        base,
        CatalogEndpointKind::OpenAiCompatible,
        ModelProtocol::ChatCompletions,
    );
    options.inference_credential =
        Some(CatalogCredential::bearer("inference-secret").unwrap());
    options.allow_insecure_localhost = true;

    let dir = tempfile::TempDir::new().unwrap();
    let cache = ModelCatalogCache::new(dir.path());
    let outcome = refresh_model_catalog_for_doctor(&options, &cache)
        .await
        .unwrap();
    server.join().unwrap();

    assert_eq!(
        outcome.credential_source,
        DoctorCatalogCredentialSource::SameOriginInference
    );
    assert_eq!(outcome.catalog.model_count, 1);
}

#[tokio::test]
async fn cross_origin_refresh_does_not_leak_inference_bearer() {
    let (address, server) = spawn_catalog_server(None);
    let mut options = DoctorCatalogRefreshOptions::new(
        Url::parse(&format!("http://localhost:{}/v1", address.port())).unwrap(),
        CatalogEndpointKind::OpenAiCompatible,
        ModelProtocol::ChatCompletions,
    );
    options.endpoint_override =
        Some(Url::parse(&format!("http://{address}/v1/models")).unwrap());
    options.inference_credential =
        Some(CatalogCredential::bearer("must-not-leak").unwrap());
    options.allow_insecure_localhost = true;

    let dir = tempfile::TempDir::new().unwrap();
    let cache = ModelCatalogCache::new(dir.path());
    let outcome = refresh_model_catalog_for_doctor(&options, &cache)
        .await
        .unwrap();
    server.join().unwrap();

    assert_eq!(
        outcome.credential_source,
        DoctorCatalogCredentialSource::None
    );
}

#[tokio::test]
async fn dedicated_bearer_is_allowed_for_cross_origin_registry() {
    let (address, server) = spawn_catalog_server(Some("authorization: bearer registry-secret"));
    let mut options = DoctorCatalogRefreshOptions::new(
        Url::parse("https://provider.example/v1").unwrap(),
        CatalogEndpointKind::OpenAiCompatible,
        ModelProtocol::ChatCompletions,
    );
    options.endpoint_override =
        Some(Url::parse(&format!("http://{address}/v1/models")).unwrap());
    options.inference_credential =
        Some(CatalogCredential::bearer("inference-secret").unwrap());
    options.dedicated_credential =
        Some(CatalogCredential::bearer("registry-secret").unwrap());
    options.allow_insecure_localhost = true;

    let dir = tempfile::TempDir::new().unwrap();
    let cache = ModelCatalogCache::new(dir.path());
    let outcome = refresh_model_catalog_for_doctor(&options, &cache)
        .await
        .unwrap();
    server.join().unwrap();

    assert_eq!(
        outcome.credential_source,
        DoctorCatalogCredentialSource::Dedicated
    );
}
