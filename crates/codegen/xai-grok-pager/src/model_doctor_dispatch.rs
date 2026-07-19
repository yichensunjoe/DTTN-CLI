//! Doctor command dispatcher.
//!
//! The existing `doctor model` path remains offline by default. Explicit model
//! catalog refreshes use a separate `doctor model-refresh` command so network
//! I/O and credential use cannot be enabled accidentally by unrelated flags.

use std::ffi::OsString;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use url::Url;
use xai_grok_sampling_types::{ApiBackend, ModelProtocol};
use xai_grok_shell::agent::config::{
    Config as AgentConfig, resolve_model_list, resolve_model_to_sampling_config,
};
use xai_grok_shell::model_catalog_doctor_refresh::{
    DoctorCatalogRefreshOptions, DoctorCatalogRefreshOutcome, refresh_model_catalog_for_doctor,
};
use xai_grok_shell::model_catalog_fetch::{CatalogCredential, CatalogEndpointKind};
use xai_grok_shell::model_catalog_runtime::default_model_catalog_cache;

#[path = "model_doctor.rs"]
mod legacy;

#[derive(Debug, Parser)]
#[command(name = "dttn", disable_help_subcommand = true)]
struct RefreshCli {
    #[command(subcommand)]
    command: RefreshRootCommand,
}

#[derive(Debug, Subcommand)]
enum RefreshRootCommand {
    /// Diagnose local DTTN configuration and runtime dependencies.
    Doctor(RefreshDoctorArgs),
}

#[derive(Debug, Args)]
struct RefreshDoctorArgs {
    #[command(subcommand)]
    command: RefreshDoctorCommand,
}

#[derive(Debug, Subcommand)]
enum RefreshDoctorCommand {
    /// Explicitly fetch model metadata and update the validated local Sidecar.
    #[command(name = "model-refresh")]
    ModelRefresh(ModelRefreshArgs),
}

#[derive(Debug, Args)]
struct ModelRefreshArgs {
    /// Model catalog key or routing model slug. Defaults to the configured model.
    #[arg(long, value_name = "MODEL")]
    model: Option<String>,

    /// Override the metadata endpoint. Otherwise DTTN derives `/models` from the Provider URL.
    #[arg(long, value_name = "URL")]
    metadata_url: Option<Url>,

    /// Payload schema exposed by the metadata endpoint.
    #[arg(long, value_enum, default_value_t = MetadataKindArg::OpenAiCompatible)]
    kind: MetadataKindArg,

    /// Environment variable containing a dedicated Bearer for a registry endpoint.
    /// The token value is never accepted as a CLI argument.
    #[arg(long, value_name = "ENV_VAR")]
    token_env: Option<String>,

    /// Metadata request timeout.
    #[arg(long, default_value_t = 5, value_name = "SECONDS")]
    timeout_secs: u64,

    /// Freshness period written into the Sidecar, capped by the fetch layer at seven days.
    #[arg(long, default_value_t = 86_400, value_name = "SECONDS")]
    cache_ttl_secs: u64,

    /// Permit plain HTTP only when the metadata endpoint is localhost or loopback.
    #[arg(long)]
    allow_insecure_localhost: bool,

    /// Emit one machine-readable JSON document.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
enum MetadataKindArg {
    OpenAiCompatible,
    DttnRegistry,
}

impl MetadataKindArg {
    fn endpoint_kind(self) -> CatalogEndpointKind {
        match self {
            Self::OpenAiCompatible => CatalogEndpointKind::OpenAiCompatible,
            Self::DttnRegistry => CatalogEndpointKind::DttnRegistry,
        }
    }
}

#[derive(Debug, Serialize)]
struct ModelRefreshReport {
    status: &'static str,
    model_key: String,
    model: String,
    kind: MetadataKindArg,
    refresh: DoctorCatalogRefreshOutcome,
}

/// Intercept the explicit refresh command and delegate every other Doctor command
/// to the established offline implementation.
pub async fn try_run_from_env() -> Result<bool> {
    let args: Vec<OsString> = std::env::args_os().collect();
    let is_refresh = args.get(1).and_then(|value| value.to_str()) == Some("doctor")
        && args.get(2).and_then(|value| value.to_str()) == Some("model-refresh");
    if !is_refresh {
        return legacy::try_run_from_env().await;
    }

    let parsed = RefreshCli::try_parse_from(args).map_err(|error| anyhow!(error.to_string()))?;
    match parsed.command {
        RefreshRootCommand::Doctor(args) => match args.command {
            RefreshDoctorCommand::ModelRefresh(args) => run_model_refresh(args).await?,
        },
    }
    Ok(true)
}

async fn run_model_refresh(args: ModelRefreshArgs) -> Result<()> {
    if args.timeout_secs == 0 {
        bail!("--timeout-secs must be greater than zero");
    }
    if args.cache_ttl_secs == 0 {
        bail!("--cache-ttl-secs must be greater than zero");
    }
    if matches!(args.kind, MetadataKindArg::DttnRegistry) && args.metadata_url.is_none() {
        bail!("--metadata-url is required with --kind dttn-registry");
    }

    let raw = xai_grok_shell::config::load_effective_config_disk_only()
        .context("failed to load effective DTTN configuration")?;
    let agent_config = AgentConfig::new_from_toml_cfg(&raw)
        .map_err(|error| anyhow!("failed to parse effective DTTN configuration: {error}"))?;
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
    let sampler = resolve_model_to_sampling_config(
        &selected,
        &models,
        None,
        None,
        Some("dttn-model-catalog-refresh".to_string()),
        None,
    )
    .ok_or_else(|| anyhow!("failed to resolve sampler configuration for '{selected}'"))?;

    let provider_base_url = Url::parse(&sampler.base_url)
        .context("configured Provider URL is invalid and cannot be used for metadata refresh")?;
    let info = entry.info();
    let mut options = DoctorCatalogRefreshOptions::new(
        provider_base_url,
        args.kind.endpoint_kind(),
        protocol_for_backend(&info.api_backend),
    );
    options.endpoint_override = args.metadata_url;
    options.timeout = Duration::from_secs(args.timeout_secs);
    options.cache_ttl = Duration::from_secs(args.cache_ttl_secs);
    options.allow_insecure_localhost = args.allow_insecure_localhost;

    // The refresh service performs the final same-origin check. Only Bearer
    // inference credentials are eligible; X-API-Key providers need a dedicated
    // registry credential until header-scheme support is added explicitly.
    if format!("{:?}", sampler.auth_scheme).eq_ignore_ascii_case("bearer")
        && let Some(token) = sampler.api_key.clone()
    {
        options.inference_credential = Some(CatalogCredential::bearer(token)?);
    }
    if let Some(env_name) = args.token_env.as_deref() {
        if env_name.trim().is_empty() {
            bail!("--token-env must name a non-empty environment variable");
        }
        let token = std::env::var(env_name)
            .map_err(|_| anyhow!("dedicated catalog token environment variable '{env_name}' is not set or is not valid Unicode"))?;
        options.dedicated_credential = Some(CatalogCredential::bearer(token)?);
    }

    let refresh = refresh_model_catalog_for_doctor(&options, &default_model_catalog_cache())
        .await
        .context("model metadata refresh failed")?;
    let report = ModelRefreshReport {
        status: "ok",
        model_key,
        model: info.model.clone(),
        kind: args.kind,
        refresh,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render_refresh_report(&report);
    }
    Ok(())
}

fn protocol_for_backend(backend: &ApiBackend) -> ModelProtocol {
    match backend {
        ApiBackend::ChatCompletions => ModelProtocol::ChatCompletions,
        ApiBackend::Responses => ModelProtocol::Responses,
        ApiBackend::Messages => ModelProtocol::AnthropicMessages,
    }
}

fn render_refresh_report(report: &ModelRefreshReport) {
    println!("DTTN model metadata refresh: OK");
    println!("  catalog key:       {}", report.model_key);
    println!("  routing model:     {}", report.model);
    println!("  schema:            {:?}", report.kind);
    println!("  endpoint:          {}", report.refresh.endpoint);
    println!(
        "  credential source: {:?}",
        report.refresh.credential_source
    );
    println!(
        "  models received:   {}",
        report.refresh.catalog.model_count
    );
    if let Some(revision) = &report.refresh.catalog.revision {
        println!("  revision:          {revision}");
    }
    println!(
        "  cache expires:     {}",
        report.refresh.catalog.expires_at_unix_ms
    );
    println!(
        "  cache path:        {}",
        report.refresh.catalog.cache_path.display()
    );
}
