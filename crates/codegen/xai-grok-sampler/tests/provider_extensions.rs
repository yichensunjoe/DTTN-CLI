//! Provider wire-contract tests for DTTN model integrations.
//!
//! These tests use a real local HTTP/SSE server. They pin the boundary between
//! standard OpenAI-compatible protocols and optional xAI private extensions.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::Json;
use axum::http::HeaderMap;
use axum::response::sse::{Event, Sse};
use axum::routing::post;
use axum::Router;
use futures_util::stream;
use indexmap::IndexMap;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

use xai_grok_sampler::{ApiBackend, RetryPolicy, SamplerActor, SamplerConfig, SamplingClient};
use xai_grok_sampling_types::{
    CompactionAtTokens, ContentPart, ConversationItem, ConversationRequest,
    ConversationToolChoice, DoomLoopRecoveryPolicy, ProviderExtensions, ToolSpec, UserItem,
};
use xai_grok_test_support::sse;

#[derive(Clone, Debug)]
struct CapturedRequest {
    headers: HeaderMap,
    body: Value,
}

struct MockServer {
    addr: SocketAddr,
    shutdown_tx: oneshot::Sender<()>,
}

impl MockServer {
    async fn spawn(app: Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        Self { addr, shutdown_tx }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
    }
}

fn config(base_url: String, backend: ApiBackend, extensions: ProviderExtensions) -> SamplerConfig {
    SamplerConfig {
        api_key: Some("test-key".into()),
        base_url,
        model: "test-model".into(),
        max_completion_tokens: Some(1024),
        temperature: None,
        top_p: None,
        api_backend: backend,
        provider_extensions: extensions,
        auth_scheme: Default::default(),
        extra_headers: IndexMap::new(),
        context_window: 128_000,
        force_http1: false,
        max_retries: Some(0),
        stream_tool_calls: false,
        idle_timeout_secs: Some(10),
        reasoning_effort: None,
        origin_client: None,
        client_identifier: Some("dttn-contract-test".into()),
        deployment_id: Some("deployment-test".into()),
        user_id: Some("user-test".into()),
        client_version: Some("0.0-test".into()),
        attribution_callback: None,
        bearer_resolver: None,
        supports_backend_search: false,
        compactions_remaining: None,
        compaction_at_tokens: None,
        doom_loop_recovery: None,
        header_injector: None,
    }
}

fn request(text: &str) -> ConversationRequest {
    ConversationRequest {
        items: vec![ConversationItem::User(UserItem {
            content: vec![ContentPart::Text {
                text: Arc::<str>::from(text),
            }],
            ..Default::default()
        })],
        x_grok_conv_id: Some("conv-test".into()),
        x_grok_req_id: Some("req-test".into()),
        x_grok_session_id: Some("session-test".into()),
        x_grok_turn_idx: Some("7".into()),
        x_grok_agent_id: Some("agent-test".into()),
        x_grok_deployment_id: Some("request-deployment".into()),
        x_grok_user_id: Some("request-user".into()),
        ..Default::default()
    }
}

fn chat_tool_call_events() -> Vec<Event> {
    vec![
        Event::default().data(
            json!({
                "id": "chatcmpl-tool",
                "object": "chat.completion.chunk",
                "created": 1,
                "model": "test-model",
                "choices": [{
                    "index": 0,
                    "delta": {
                        "role": "assistant",
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_1",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":"
                            }
                        }]
                    },
                    "finish_reason": null
                }]
            })
            .to_string(),
        ),
        Event::default().data(
            json!({
                "id": "chatcmpl-tool",
                "object": "chat.completion.chunk",
                "created": 1,
                "model": "test-model",
                "choices": [{
                    "index": 0,
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "function": { "arguments": "\"README.md\"}" }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })
            .to_string(),
        ),
        Event::default().data(
            json!({
                "id": "chatcmpl-tool",
                "object": "chat.completion.chunk",
                "created": 1,
                "model": "test-model",
                "choices": [],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 4,
                    "total_tokens": 14
                }
            })
            .to_string(),
        ),
        Event::default().data("[DONE]"),
    ]
}

async fn collect(
    cfg: SamplerConfig,
    request: ConversationRequest,
) -> xai_grok_sampling_types::ConversationResponse {
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(cfg, RetryPolicy::default(), event_tx);
    let (response, _) = handle
        .submit_and_collect("provider-contract".into(), request)
        .await
        .expect("provider contract request should complete");
    response
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standard_provider_sends_no_x_grok_headers() {
    let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let captured_handler = Arc::clone(&captured);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let captured = Arc::clone(&captured_handler);
            async move {
                captured.lock().unwrap().push(CapturedRequest { headers, body });
                Sse::new(stream::iter(
                    sse::chat_completion_events("ok", "test-model")
                        .into_iter()
                        .map(Ok::<_, std::convert::Infallible>),
                ))
            }
        }),
    );
    let server = MockServer::spawn(app).await;

    let response = collect(
        config(
            server.base_url(),
            ApiBackend::ChatCompletions,
            ProviderExtensions::Standard,
        ),
        request("hello"),
    )
    .await;
    assert_eq!(response.assistant_text(), "ok");
    server.shutdown();

    let captured = captured.lock().unwrap();
    let request = captured.first().expect("request captured");
    assert!(
        request
            .headers
            .keys()
            .all(|name| !name.as_str().starts_with("x-grok-"))
    );
    assert!(
        request
            .headers
            .get("user-agent")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("dttn-cli/")
    );
    assert_eq!(
        request.headers.get("authorization").unwrap(),
        "Bearer test-key"
    );
    assert!(request.body.get("stream_tool_calls").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn xai_provider_preserves_private_tracking_headers() {
    let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let captured_handler = Arc::clone(&captured);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let captured = Arc::clone(&captured_handler);
            async move {
                captured.lock().unwrap().push(CapturedRequest { headers, body });
                Sse::new(stream::iter(
                    sse::chat_completion_events("ok", "test-model")
                        .into_iter()
                        .map(Ok::<_, std::convert::Infallible>),
                ))
            }
        }),
    );
    let server = MockServer::spawn(app).await;

    collect(
        config(
            server.base_url(),
            ApiBackend::ChatCompletions,
            ProviderExtensions::Xai,
        ),
        request("hello"),
    )
    .await;
    server.shutdown();

    let captured = captured.lock().unwrap();
    let headers = &captured.first().expect("request captured").headers;
    assert_eq!(headers.get("x-grok-client-identifier").unwrap(), "dttn-contract-test");
    assert_eq!(headers.get("x-grok-client-version").unwrap(), "0.0-test");
    assert_eq!(headers.get("x-grok-conv-id").unwrap(), "conv-test");
    assert_eq!(headers.get("x-grok-req-id").unwrap(), "req-test");
    assert_eq!(headers.get("x-grok-model-override").unwrap(), "test-model");
    assert_eq!(headers.get("x-grok-session-id").unwrap(), "session-test");
    assert_eq!(headers.get("x-grok-turn-idx").unwrap(), "7");
}

#[test]
fn standard_provider_rejects_private_capabilities_before_network_io() {
    let base = || {
        config(
            "http://127.0.0.1:1/v1".into(),
            ApiBackend::Responses,
            ProviderExtensions::Standard,
        )
    };

    let mut cases: Vec<(&str, SamplerConfig)> = Vec::new();

    let mut stream_tools = base();
    stream_tools.stream_tool_calls = true;
    cases.push(("stream_tool_calls", stream_tools));

    let mut search = base();
    search.supports_backend_search = true;
    cases.push(("backend search", search));

    let mut compact = base();
    compact.compaction_at_tokens = Some(CompactionAtTokens::Fixed(1000));
    cases.push(("server compaction", compact));

    let mut doom_loop = base();
    doom_loop.doom_loop_recovery = Some(DoomLoopRecoveryPolicy::default());
    cases.push(("server doom-loop", doom_loop));

    let mut private_header = base();
    private_header
        .extra_headers
        .insert("x-grok-private".into(), "1".into());
    cases.push(("xAI private headers", private_header));

    for (expected, cfg) in cases {
        let error = SamplingClient::new(cfg).expect_err("standard provider must reject private mode");
        assert!(
            error.to_string().contains(expected),
            "expected {expected:?} in {error}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standard_chat_stream_reassembles_tool_call_arguments() {
    let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let captured_handler = Arc::clone(&captured);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let captured = Arc::clone(&captured_handler);
            async move {
                captured.lock().unwrap().push(CapturedRequest { headers, body });
                Sse::new(stream::iter(
                    chat_tool_call_events()
                        .into_iter()
                        .map(Ok::<_, std::convert::Infallible>),
                ))
            }
        }),
    );
    let server = MockServer::spawn(app).await;

    let mut req = request("read the readme");
    req.tools = vec![ToolSpec {
        name: "read_file".into(),
        description: Some("Read a UTF-8 text file".into()),
        parameters: json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        }),
    }];
    req.tool_choice = Some(ConversationToolChoice::Required);

    let response = collect(
        config(
            server.base_url(),
            ApiBackend::ChatCompletions,
            ProviderExtensions::Standard,
        ),
        req,
    )
    .await;
    server.shutdown();

    let assistant = response.assistant().expect("assistant response");
    assert_eq!(assistant.tool_calls.len(), 1);
    assert_eq!(assistant.tool_calls[0].name, "read_file");
    assert_eq!(assistant.tool_calls[0].arguments.as_ref(), "{\"path\":\"README.md\"}");

    let captured = captured.lock().unwrap();
    let body = &captured.first().expect("request captured").body;
    assert_eq!(body["tools"][0]["function"]["name"], "read_file");
    assert!(body.get("stream_tool_calls").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standard_responses_stream_works_without_private_fields() {
    let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
    let captured_handler = Arc::clone(&captured);
    let app = Router::new().route(
        "/v1/responses",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let captured = Arc::clone(&captured_handler);
            async move {
                captured.lock().unwrap().push(CapturedRequest { headers, body });
                Sse::new(stream::iter(
                    sse::responses_api_events("responses ok", "test-model")
                        .into_iter()
                        .map(Ok::<_, std::convert::Infallible>),
                ))
            }
        }),
    );
    let server = MockServer::spawn(app).await;

    let response = collect(
        config(
            server.base_url(),
            ApiBackend::Responses,
            ProviderExtensions::Standard,
        ),
        request("hello"),
    )
    .await;
    server.shutdown();

    assert_eq!(response.assistant_text(), "responses ok");
    let captured = captured.lock().unwrap();
    let request = captured.first().expect("request captured");
    assert!(request.body.get("stream_tool_calls").is_none());
    assert!(
        request
            .headers
            .keys()
            .all(|name| !name.as_str().starts_with("x-grok-"))
    );
}
