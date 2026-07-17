//! Responses API Tool Calling contracts for standard DTTN providers.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::Json;
use axum::response::sse::{Event, Sse};
use axum::routing::post;
use axum::Router;
use futures_util::stream;
use indexmap::IndexMap;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

use xai_grok_sampler::{ApiBackend, RetryPolicy, SamplerActor, SamplerConfig};
use xai_grok_sampling_types::{
    AssistantItem, ContentPart, ConversationItem, ConversationRequest, ProviderExtensions,
    ToolCall, ToolResultItem, UserItem,
};
use xai_grok_test_support::sse;

struct MockServer {
    addr: SocketAddr,
    shutdown_tx: oneshot::Sender<()>,
}

impl MockServer {
    async fn spawn(app: Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
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

fn config(base_url: String) -> SamplerConfig {
    SamplerConfig {
        api_key: Some("test-key".into()),
        base_url,
        model: "test-model".into(),
        max_completion_tokens: Some(1024),
        temperature: None,
        top_p: None,
        api_backend: ApiBackend::Responses,
        provider_extensions: ProviderExtensions::Standard,
        auth_scheme: Default::default(),
        extra_headers: IndexMap::new(),
        context_window: 128_000,
        force_http1: false,
        max_retries: Some(0),
        stream_tool_calls: false,
        idle_timeout_secs: Some(10),
        reasoning_effort: None,
        origin_client: None,
        client_identifier: Some("dttn-responses-contract".into()),
        deployment_id: None,
        user_id: None,
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

fn user_item(text: &str) -> ConversationItem {
    ConversationItem::User(UserItem {
        content: vec![ContentPart::Text {
            text: Arc::<str>::from(text),
        }],
        ..Default::default()
    })
}

fn responses_tool_call_events() -> Vec<Event> {
    let completed_response = json!({
        "id": "resp_tool",
        "object": "response",
        "created_at": 1,
        "model": "test-model",
        "status": "completed",
        "output": [{
            "type": "function_call",
            "id": "fc_1",
            "call_id": "call_1",
            "name": "read_file",
            "arguments": "{\"path\":\"README.md\"}",
            "status": "completed"
        }],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 4,
            "total_tokens": 14,
            "input_tokens_details": { "cached_tokens": 0 },
            "output_tokens_details": { "reasoning_tokens": 0 }
        }
    });

    vec![
        Event::default().data(
            json!({
                "type": "response.created",
                "sequence_number": 0,
                "response": {
                    "id": "resp_tool",
                    "object": "response",
                    "created_at": 1,
                    "model": "test-model",
                    "status": "in_progress",
                    "output": []
                }
            })
            .to_string(),
        ),
        Event::default().data(
            json!({
                "type": "response.output_item.added",
                "sequence_number": 1,
                "output_index": 0,
                "item": {
                    "type": "function_call",
                    "id": "fc_1",
                    "call_id": "call_1",
                    "name": "read_file",
                    "arguments": "",
                    "status": "in_progress"
                }
            })
            .to_string(),
        ),
        Event::default().data(
            json!({
                "type": "response.function_call_arguments.delta",
                "sequence_number": 2,
                "item_id": "fc_1",
                "output_index": 0,
                "delta": "{\"path\":"
            })
            .to_string(),
        ),
        Event::default().data(
            json!({
                "type": "response.function_call_arguments.delta",
                "sequence_number": 3,
                "item_id": "fc_1",
                "output_index": 0,
                "delta": "\"README.md\"}"
            })
            .to_string(),
        ),
        Event::default().data(
            json!({
                "type": "response.function_call_arguments.done",
                "sequence_number": 4,
                "item_id": "fc_1",
                "output_index": 0,
                "name": "read_file",
                "arguments": "{\"path\":\"README.md\"}"
            })
            .to_string(),
        ),
        Event::default().data(
            json!({
                "type": "response.output_item.done",
                "sequence_number": 5,
                "output_index": 0,
                "item": completed_response["output"][0]
            })
            .to_string(),
        ),
        Event::default().data(
            json!({
                "type": "response.completed",
                "sequence_number": 6,
                "response": completed_response
            })
            .to_string(),
        ),
        Event::default().data("[DONE]"),
    ]
}

async fn collect(
    cfg: SamplerConfig,
    request_id: &str,
    request: ConversationRequest,
) -> xai_grok_sampling_types::ConversationResponse {
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(
        cfg,
        RetryPolicy {
            max_retries: 0,
            rate_limit_retry_threshold: 0,
        },
        event_tx,
    );
    let (response, _) = handle
        .submit_and_collect(request_id.into(), request)
        .await
        .expect("responses contract request should complete");
    response
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standard_responses_reassembles_function_call_arguments() {
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured_handler = Arc::clone(&captured);
    let app = Router::new().route(
        "/v1/responses",
        post(move |Json(body): Json<Value>| {
            let captured = Arc::clone(&captured_handler);
            async move {
                captured.lock().unwrap().push(body);
                Sse::new(stream::iter(
                    responses_tool_call_events()
                        .into_iter()
                        .map(Ok::<_, std::convert::Infallible>),
                ))
            }
        }),
    );
    let server = MockServer::spawn(app).await;

    let response = collect(
        config(server.base_url()),
        "responses-tool-call",
        ConversationRequest {
            items: vec![user_item("Read README.md")],
            ..Default::default()
        },
    )
    .await;
    server.shutdown();

    let assistant = response.assistant().expect("assistant item");
    assert_eq!(assistant.tool_calls.len(), 1);
    assert_eq!(assistant.tool_calls[0].id.as_ref(), "call_1");
    assert_eq!(assistant.tool_calls[0].name, "read_file");
    assert_eq!(
        assistant.tool_calls[0].arguments.as_ref(),
        "{\"path\":\"README.md\"}"
    );

    let request = &captured.lock().unwrap()[0];
    assert_eq!(request["stream"], true);
    assert!(request.get("stream_tool_calls").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standard_responses_accepts_tool_result_on_followup_turn() {
    let request_count = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let count_handler = Arc::clone(&request_count);
    let captured_handler = Arc::clone(&captured);

    let app = Router::new().route(
        "/v1/responses",
        post(move |Json(body): Json<Value>| {
            let attempt = count_handler.fetch_add(1, Ordering::SeqCst);
            let captured = Arc::clone(&captured_handler);
            async move {
                captured.lock().unwrap().push(body);
                let events = if attempt == 0 {
                    responses_tool_call_events()
                } else {
                    sse::responses_api_events("tool result accepted", "test-model")
                };
                Sse::new(stream::iter(
                    events
                        .into_iter()
                        .map(Ok::<_, std::convert::Infallible>),
                ))
            }
        }),
    );
    let server = MockServer::spawn(app).await;
    let cfg = config(server.base_url());

    let first = collect(
        cfg.clone(),
        "responses-first",
        ConversationRequest {
            items: vec![user_item("Read README.md")],
            ..Default::default()
        },
    )
    .await;
    let first_assistant = first.assistant().expect("assistant tool call");
    let call = first_assistant.tool_calls.first().expect("tool call");

    let followup = ConversationRequest {
        items: vec![
            user_item("Read README.md"),
            ConversationItem::Assistant(AssistantItem {
                content: Arc::<str>::from(""),
                tool_calls: vec![ToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                }],
                model_id: Some("test-model".to_string()),
                model_fingerprint: None,
                reasoning_effort: None,
            }),
            ConversationItem::ToolResult(ToolResultItem {
                tool_call_id: call.id.to_string(),
                content: Arc::<str>::from("DTTN README CONTENT"),
                images: Vec::new(),
            }),
        ],
        ..Default::default()
    };

    let second = collect(cfg, "responses-followup", followup).await;
    server.shutdown();

    assert_eq!(second.assistant_text(), "tool result accepted");
    assert_eq!(request_count.load(Ordering::SeqCst), 2);

    let bodies = captured.lock().unwrap();
    let second_input = bodies[1]["input"]
        .as_array()
        .expect("responses input array");
    assert!(second_input.iter().any(|item| {
        item["type"] == "function_call"
            && item["call_id"] == "call_1"
            && item["name"] == "read_file"
    }));
    assert!(second_input.iter().any(|item| {
        item["type"] == "function_call_output"
            && item["call_id"] == "call_1"
            && item["output"] == "DTTN README CONTENT"
    }));
}
