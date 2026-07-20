//! Offline user configuration commands.

use anyhow::Context as _;
use clap::{Args, Subcommand, ValueEnum};

#[derive(Debug, Clone, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: Option<ConfigCommand>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ConfigCommand {
    /// Show or change the persistent default model for new sessions
    Model {
        /// Model ID to persist. Omit to show the current values.
        #[arg(value_name = "MODEL", conflicts_with = "reset")]
        model: Option<String>,
        /// Remove the user-level model override.
        #[arg(long)]
        reset: bool,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// List model providers or register a custom OpenAI-compatible model
    Models(ModelsArgs),
    /// Print the user configuration file path
    Path {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Args)]
pub struct ModelsArgs {
    #[command(subcommand)]
    command: Option<ModelsCommand>,
    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Debug, Clone, Subcommand)]
enum ModelsCommand {
    /// Register a custom OpenAI-compatible provider model
    Custom(CustomModelArgs),
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CustomBackend {
    /// OpenAI Chat Completions-compatible endpoint
    ChatCompletions,
    /// OpenAI Responses-compatible endpoint
    Responses,
}

impl From<CustomBackend> for xai_grok_config::user_config::CustomModelApiBackend {
    fn from(value: CustomBackend) -> Self {
        match value {
            CustomBackend::ChatCompletions => Self::ChatCompletions,
            CustomBackend::Responses => Self::Responses,
        }
    }
}

#[derive(Debug, Clone, Args)]
struct CustomModelArgs {
    /// Stable lowercase provider ID used in the provider/model reference
    #[arg(value_name = "PROVIDER")]
    provider: String,
    /// Model slug sent to the provider API
    #[arg(value_name = "MODEL")]
    model: String,
    /// Human-readable model name
    #[arg(long, value_name = "NAME")]
    name: Option<String>,
    /// Provider inference base URL
    #[arg(long, value_name = "URL")]
    base_url: String,
    /// Environment variable containing the API key; the key itself is never persisted
    #[arg(long, value_name = "ENV_VAR")]
    api_key_env: String,
    /// Compatible request protocol
    #[arg(long, value_enum, default_value = "chat-completions")]
    backend: CustomBackend,
    /// Total model context window in tokens
    #[arg(long, value_name = "TOKENS")]
    context_window: u64,
    /// Maximum completion tokens, when known
    #[arg(long, value_name = "TOKENS")]
    max_completion_tokens: Option<u32>,
    /// Explicitly make this model the default for new sessions
    #[arg(long)]
    set_default: bool,
}

pub fn run(args: ConfigArgs) -> anyhow::Result<()> {
    match args.command {
        Some(ConfigCommand::Model { model, reset, json }) => {
            run_model(model.as_deref(), reset, json)
        }
        Some(ConfigCommand::Models(args)) => run_models(args),
        Some(ConfigCommand::Path { json }) => run_path(json),
        None => run_model(None, false, false),
    }
}

fn run_path(json: bool) -> anyhow::Result<()> {
    let path = xai_grok_config::user_config::user_config_path();
    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({ "path": path }))?
        );
    } else {
        println!("{}", path.display());
    }
    Ok(())
}

fn run_models(args: ModelsArgs) -> anyhow::Result<()> {
    match args.command {
        Some(ModelsCommand::Custom(custom)) => run_custom_model(custom, args.json),
        None => list_model_providers(args.json),
    }
}

fn list_model_providers(json: bool) -> anyhow::Result<()> {
    let providers = xai_grok_config::model_providers::model_providers();
    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "providers": providers,
                "modelRefFormat": "provider/model",
                "customProvider": {
                    "command": "dttn config models custom <PROVIDER> <MODEL> --base-url <URL> --api-key-env <ENV_VAR> --context-window <TOKENS>",
                    "supportedBackends": ["chat_completions", "responses"],
                    "changesDefaultOnlyWith": "--set-default",
                },
            }))?
        );
        return Ok(());
    }

    println!("DTTN model providers");
    println!();
    for provider in providers {
        let auth = if provider.auth_env.is_empty() {
            "user-defined".to_owned()
        } else {
            provider.auth_env.join(", ")
        };
        println!("  {:<14} {}", provider.id, provider.name);
        println!("  {:<14} API: {}; auth: {}", "", provider.api_style, auth);
    }
    println!();
    println!("Model references use `provider/model`.");
    println!("Register a custom OpenAI-compatible model:");
    println!(
        "  dttn config models custom <PROVIDER> <MODEL> --base-url <URL> --api-key-env <ENV_VAR> --context-window <TOKENS>"
    );
    println!("Add `--set-default` only when the new model should become the default for new sessions.");
    println!(
        "Provider-native Gemini and Ollama protocols are listed for discovery but are not configured by the custom OpenAI-compatible command."
    );
    Ok(())
}

fn run_custom_model(args: CustomModelArgs, json: bool) -> anyhow::Result<()> {
    let config = xai_grok_config::user_config::CustomModelConfig {
        provider_id: args.provider,
        model_id: args.model,
        display_name: args.name,
        base_url: args.base_url,
        api_key_env: args.api_key_env,
        api_backend: args.backend.into(),
        context_window: args.context_window,
        max_completion_tokens: args.max_completion_tokens,
        set_default: args.set_default,
    };
    let result = xai_grok_config::user_config::set_custom_model(&config)
        .context("failed to register custom provider model")?;

    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "action": "registered",
                "modelRef": result.model_ref,
                "path": result.path,
                "defaultChanged": result.default_changed,
                "appliesTo": "new_sessions",
                "resumedSessionsUseFrozenModel": true,
            }))?
        );
        return Ok(());
    }

    println!("Registered custom model {}.", result.model_ref);
    println!("Config file: {}", result.path.display());
    if result.default_changed {
        println!("This model is now the default for new sessions.");
    } else {
        println!("The current default was not changed.");
        println!(
            "Use `dttn config model {}` to select it explicitly for new sessions.",
            result.model_ref
        );
    }
    println!("Resumed sessions continue using their frozen model contract.");
    Ok(())
}

fn run_model(model: Option<&str>, reset: bool, json: bool) -> anyhow::Result<()> {
    let path = xai_grok_config::user_config::user_config_path();
    let action = if let Some(model) = model {
        xai_grok_config::user_config::set_user_default_model(model)
            .context("failed to set persistent default model")?;
        "set"
    } else if reset {
        let removed = xai_grok_config::user_config::reset_user_default_model()
            .context("failed to reset persistent default model")?;
        if removed { "reset" } else { "unchanged" }
    } else {
        "show"
    };

    let user_default = xai_grok_config::user_config::user_default_model()
        .context("failed to read user default model")?;
    let effective_default = effective_default_model()?;

    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "action": action,
                "path": path,
                "userDefault": user_default,
                "effectiveDefault": effective_default,
                "appliesTo": "new_sessions",
                "resumedSessionsUseFrozenModel": true,
            }))?
        );
        return Ok(());
    }

    match action {
        "set" => println!(
            "Persistent default model set to {}.",
            user_default.as_deref().unwrap_or("<unset>")
        ),
        "reset" => println!("Removed the user-level default model override."),
        "unchanged" => println!("No user-level default model override was set."),
        _ => {}
    }
    println!(
        "User default:      {}",
        display_model(user_default.as_deref())
    );
    println!(
        "Effective default: {}",
        display_model(effective_default.as_deref())
    );
    println!("Config file:       {}", path.display());
    println!("Applies to new sessions; resumed sessions keep their frozen model.");
    if model.is_some() {
        println!("Use `dttn models` to inspect available catalog models.");
    }
    Ok(())
}

fn effective_default_model() -> anyhow::Result<Option<String>> {
    let config = xai_grok_config::load_effective_config_disk_only()
        .context("failed to load effective DTTN configuration")?;
    Ok(config
        .get("models")
        .and_then(|models| models.get("default"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned))
}

fn display_model(model: Option<&str>) -> &str {
    model.unwrap_or("<automatic>")
}
