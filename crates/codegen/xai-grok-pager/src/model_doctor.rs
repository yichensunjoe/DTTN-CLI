//! Model-provider diagnostics for `dttn doctor model`.
//!
//! The default path is offline: it resolves the effective model catalog and
//! validates the sampler configuration without performing network I/O. `--live`
//! explicitly enables bounded text-stream and tool-calling probes.

use std::ffi::OsString;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use tokio::sync::mpsc;
use xai_grok_shell::agent::config::{
    Config as AgentConfig, find_model_by_id, resolve_model_list, resolve_model_to_sampling_config,
};
use xai_grok_shell::sampling::{
    ContentPart, ConversationItem, ConversationRequest, ConversationToolChoice, RequestId,
    RetryPolicy, SamplerActor, SamplingClient, SamplingEvent, ToolSpec, UserItem,
};

#[derive(Debug, Parser)]
#[command(name = "dttn", disable_help_subcommand = true)]
struct DoctorCli {
    #[command(subcommand)]
    command: DoctorRootCommand,
}

#[derive(Debug, Subcommand)]
enum DoctorRootCommand {
    /// Diagnose local DTTN configuration and runtime dependencies.
    Doctor(DoctorArgs),
}

#[derive(Debug, Args)]
struct DoctorArgs {
    #[command(subcommand)]
    command: DoctorCommand,
}

#[derive(Debug, Subcommand)]
enum DoctorCommand {
    /// Audit one model and optionally run bounded live protocol probes.
    Model(ModelDoctorArgs),
}

#[derive(Debug, Args)]
struct ModelDoctorArgs {
    /// Model catalog key or routing model slug. Defaults to the configured model.
    #[arg(long, value_name = "MODEL")]
    model: Option<String>,

    /// Perform real inference requests. Without this flag, no model request is sent.
    #[arg(long)]
    live: bool,

    /// Emit one machine-readable JSON document.
    #[arg(long)]
    json: bool,

    /// Per-probe wall-clock timeout.
    #[arg(long, default_value_t = 30, value_name = "SECONDS")]
    timeout_secs: u64,

    /// Skip the forced Tool Calling probe.
    #[arg(long)]
    skip_tool_call: bool,
}

#[derive(Debug, Serialize)]
struct ModelDoctorReport {
    status: &'static str,
    model_key: String,
    model: String,
    endpoint: String,
    api_backend: String,
    provider_extensions: String,
    auth_scheme: String,
    auth_configured: bool,
    context_window: u64,
    max_completion_tokens: Option<u32>,
    max_retries: Option<u32>,
    inference_idle_timeout_secs: Option<u64>,
    agent_type: String,
    supports_backend_search: bool,
    stream_tool_calls: bool,
    configuration_valid: bool,
    warnings: Vec<String>,
    live: Option<LiveReport>,
}

#[derive(Debug, Serialize)]
struct LiveReport {
    text: ProbeReport,
    tool_call: Option<ProbeReport>,
}

#[derive(Debug, Serialize)]
struct ProbeReport {
    ok: bool,
    total_latency_ms: u128,
    first_token_ms: Option<u128>,
    text_non_empty: bool,
    contract_match: bool,
    tool_calls: Vec<ToolCallReport>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct ToolCallReport {
    name: String,
    arguments_valid_json: bool,
}

/// Intercept only the `doctor` command. All other CLI invocations remain owned by
/// the pager's canonical clap parser.
pub async fn try_run_from_env() -> Result<bool> {
    let args: Vec<OsString> = std::env::args_os().collect();
    if args.get(1).and_then(|v| v.to_str()) != Some("doctor") {
        return Ok(false);
    }

    let parsed = DoctorCli::try_parse_from(args).map_err(|error| anyhow!(error.to_string()))?;
    match parsed.command {
        DoctorRootCommand::Doctor(args) => match args.command {
            DoctorCommand::Model(args) => run_model_doctor(args).await?,
        },
    }
    Ok(true)
}

async fn run_model_doctor(args: ModelDoctorArgs) -> Result<()> {
    if args.timeout_secs == 0 {
        bail!("--timeout-secs must be greater than zero");
    }

    let raw = xai_grok_shell::config::load_effective_config_disk_only()
        .context("failed to load effective DTTN configuration")?;
    let agent_config = AgentConfig::new_from_toml_cfg(&raw)
        .context("failed to parse effective DTTN configuration")?;
    let models = resolve_model_list(&agent_config, None);

    let selected = args
        .model
        .clone()
        .or_else(|| agent_config.models.default.clone())
        .or_else(|| {
            models
                .iter()
                .find(|(_, entry)| entry.info().user_selectable)
                .map(|(key, _)| key.clone())
        })
        .ok_or_else(|| anyhow!("no model is configured or selectable"))?;

    let (model_key, entry) = models
        .iter()
        .find(|(key, entry)| *key == &selected || entry.info().model == selected)
        .map(|(key, entry)| (key.clone(), entry))
        .ok_or_else(|| anyhow!("model '{selected}' was not found in the effective catalog"))?;

    let mut sampler = resolve_model_to_sampling_config(
        &selected,
        &models,
        None,
        None,
        Some("dttn-model-doctor".to_string()),
        None,
    )
    .ok_or_else(|| anyhow!("failed to resolve sampler configuration for '{selected}'"))?;

    // Doctor probes must be bounded and expose the first failure instead of
    // spending minutes behind the runtime's normal retry policy.
    sampler.max_retries = Some(0);
    sampler.idle_timeout_secs = Some(args.timeout_secs);

    let info = entry.info();
    let mut warnings = Vec::new();
    if sampler.api_key.is_none() {
        warnings.push(
            "no API credential resolved; configure api_key/env_key before using --live".to_string(),
        );
    }
    if info.context_window.get() == 200_000 {
        warnings.push(
            "context_window is 200000, which may be the generic fallback rather than a verified provider limit"
                .to_string(),
        );
    }
    if info.supports_backend_search {
        warnings.push(
            "backend search is provider-hosted; verify the endpoint implements the declared extension"
                .to_string(),
        );
    }

    let configuration_error = SamplingClient::new(sampler.clone()).err().map(|error| error.to_string());
    if let Some(error) = &configuration_error {
        warnings.push(format!("sampler configuration rejected: {error}"));
    }

    let live = if args.live {
        if configuration_error.is_some() {
            None
        } else if sampler.api_key.is_none() {
            warnings.push("live probes skipped because no credential was resolved".to_string());
            None
        } else {
            let timeout = Duration::from_secs(args.timeout_secs);
            let text = run_probe(
                sampler.clone(),
                text_probe_request(),
                RequestId::from("doctor-model-text"),
                timeout,
                ProbeExpectation::TextMarker("DTTN_MODEL_OK"),
            )
            .await;
            let tool_call = if args.skip_tool_call {
                None
            } else {
                Some(
                    run_probe(
                        sampler.clone(),
                        tool_probe_request(),
                        RequestId::from("doctor-model-tool"),
                        timeout,
                        ProbeExpectation::Tool("dttn_model_probe"),
                    )
                    .await,
                )
            };
            Some(LiveReport { text, tool_call })
        }
    } else {
        None
    };

    let live_ok = live.as_ref().is_none_or(|report| {
        report.text.ok && report.tool_call.as_ref().is_none_or(|probe| probe.ok)
    });
    let configuration_valid = configuration_error.is_none();
    let status = if configuration_valid && live_ok && warnings.is_empty() {
        "ok"
    } else if configuration_valid && live_ok {
        "warning"
    } else {
        "error"
    };

    let report = ModelDoctorReport {
        status,
        model_key,
        model: info.model.clone(),
        endpoint: redact_endpoint(&sampler.base_url),
        api_backend: format!("{:?}", info.api_backend).to_ascii_lowercase(),
        provider_extensions: format!("{:?}", info.provider_extensions).to_ascii_lowercase(),
        auth_scheme: format!("{:?}", sampler.auth_scheme).to_ascii_lowercase(),
        auth_configured: sampler.api_key.is_some(),
        context_window: info.context_window.get(),
        max_completion_tokens: info.max_completion_tokens,
        max_retries: info.max_retries,
        inference_idle_timeout_secs: info.inference_idle_timeout_secs,
        agent_type: info.agent_type.clone(),
        supports_backend_search: info.supports_backend_search,
        stream_tool_calls: info.stream_tool_calls.unwrap_or(false),
        configuration_valid,
        warnings,
        live,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render_human(&report);
    }

    if report.status == "error" {
        bail!("model doctor detected an invalid or failed model integration");
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum ProbeExpectation {
    TextMarker(&'static str),
    Tool(&'static str),
}

async fn run_probe(
    config: xai_grok_shell::sampling::SamplerConfig,
    request: ConversationRequest,
    request_id: RequestId,
    timeout: Duration,
    expectation: ProbeExpectation,
) -> ProbeReport {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let handle = SamplerActor::spawn(config, RetryPolicy::default(), event_tx);
    let started = Instant::now();
    let mut first_token_ms = None;
    handle.submit(request_id.clone(), request);

    let outcome = tokio::time::timeout(timeout, async {
        loop {
            let Some(event) = event_rx.recv().await else {
                return Err("sampler event channel closed before a terminal event".to_string());
            };
            match event {
                SamplingEvent::FirstToken { .. } => {
                    first_token_ms.get_or_insert_with(|| started.elapsed().as_millis());
                }
                SamplingEvent::Completed { response, .. } => return Ok(response),
                SamplingEvent::Failed { error, .. } => return Err(error.message),
                _ => {}
            }
        }
    })
    .await;

    let total_latency_ms = started.elapsed().as_millis();
    match outcome {
        Err(_) => {
            handle.cancel(request_id);
            ProbeReport {
                ok: false,
                total_latency_ms,
                first_token_ms,
                text_non_empty: false,
                contract_match: false,
                tool_calls: Vec::new(),
                error: Some(format!("probe timed out after {} seconds", timeout.as_secs())),
            }
        }
        Ok(Err(error)) => ProbeReport {
            ok: false,
            total_latency_ms,
            first_token_ms,
            text_non_empty: false,
            contract_match: false,
            tool_calls: Vec::new(),
            error: Some(error),
        },
        Ok(Ok(response)) => {
            let assistant = response.assistant();
            let text = assistant
                .map(|item| item.content.as_ref())
                .unwrap_or_default();
            let tool_calls = assistant
                .map(|item| {
                    item.tool_calls
                        .iter()
                        .map(|call| ToolCallReport {
                            name: call.name.clone(),
                            arguments_valid_json: serde_json::from_str::<serde_json::Value>(
                                call.arguments.as_ref(),
                            )
                            .is_ok(),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let contract_match = match expectation {
                ProbeExpectation::TextMarker(marker) => text.contains(marker),
                ProbeExpectation::Tool(name) => tool_calls.iter().any(|call| call.name == name),
            };
            ProbeReport {
                ok: contract_match,
                total_latency_ms,
                first_token_ms,
                text_non_empty: !text.trim().is_empty(),
                contract_match,
                tool_calls,
                error: (!contract_match).then(|| "provider response did not satisfy the probe contract".to_string()),
            }
        }
    }
}

fn text_probe_request() -> ConversationRequest {
    ConversationRequest {
        items: vec![ConversationItem::User(UserItem {
            content: vec![ContentPart::Text {
                text: Arc::<str>::from(
                    "Reply with exactly DTTN_MODEL_OK. Do not call a tool and do not add other text.",
                ),
            }],
            ..Default::default()
        })],
        ..Default::default()
    }
}

fn tool_probe_request() -> ConversationRequest {
    ConversationRequest {
        items: vec![ConversationItem::User(UserItem {
            content: vec![ContentPart::Text {
                text: Arc::<str>::from(
                    "Call the dttn_model_probe tool with value set to DTTN_TOOL_OK. Do not answer in plain text.",
                ),
            }],
            ..Default::default()
        })],
        tools: vec![ToolSpec {
            name: "dttn_model_probe".to_string(),
            description: Some("DTTN model-provider Tool Calling contract probe".to_string()),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                },
                "required": ["value"],
                "additionalProperties": false
            }),
        }],
        tool_choice: Some(ConversationToolChoice::Required),
        ..Default::default()
    }
}

fn redact_endpoint(value: &str) -> String {
    let Ok(mut url) = url::Url::parse(value) else {
        return "<invalid-url>".to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string().trim_end_matches('/').to_string()
}

fn render_human(report: &ModelDoctorReport) {
    println!("DTTN model doctor: {}", report.status.to_ascii_uppercase());
    println!("  catalog key:          {}", report.model_key);
    println!("  routing model:        {}", report.model);
    println!("  endpoint:             {}", report.endpoint);
    println!("  API backend:          {}", report.api_backend);
    println!("  provider extensions:  {}", report.provider_extensions);
    println!("  auth scheme:          {}", report.auth_scheme);
    println!("  credential resolved:  {}", report.auth_configured);
    println!("  context window:       {}", report.context_window);
    println!("  agent type:           {}", report.agent_type);
    println!("  config valid:         {}", report.configuration_valid);

    if !report.warnings.is_empty() {
        println!("  warnings:");
        for warning in &report.warnings {
            println!("    - {warning}");
        }
    }

    if let Some(live) = &report.live {
        render_probe("text stream", &live.text);
        if let Some(tool_call) = &live.tool_call {
            render_probe("tool calling", tool_call);
        }
    }
}

fn render_probe(name: &str, probe: &ProbeReport) {
    println!("  {name}: {}", if probe.ok { "PASS" } else { "FAIL" });
    println!("    total latency:      {} ms", probe.total_latency_ms);
    if let Some(first_token_ms) = probe.first_token_ms {
        println!("    first token:        {first_token_ms} ms");
    }
    if !probe.tool_calls.is_empty() {
        println!("    tool calls:");
        for call in &probe.tool_calls {
            println!(
                "      - {} (arguments JSON: {})",
                call.name, call.arguments_valid_json
            );
        }
    }
    if let Some(error) = &probe.error {
        println!("    error:              {error}");
    }
}
