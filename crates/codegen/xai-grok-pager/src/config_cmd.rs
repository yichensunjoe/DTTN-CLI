//! Offline user configuration commands.

use anyhow::Context as _;
use clap::{Args, Subcommand};

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
    /// Print the user configuration file path
    Path {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
}

pub fn run(args: ConfigArgs) -> anyhow::Result<()> {
    match args.command {
        Some(ConfigCommand::Model { model, reset, json }) => {
            run_model(model.as_deref(), reset, json)
        }
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
