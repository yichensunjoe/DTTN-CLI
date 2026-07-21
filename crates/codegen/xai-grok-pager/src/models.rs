//! `dttn models` subcommand - list configured models / set the default.
//!
//! Mirrors OpenClaw's `models list` / `models set` verbs. `list` reads the
//! configured `[model.*]` entries from the user config file (offline, no agent
//! spawn); `set` picks the default for new sessions.

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use toml_edit::DocumentMut;

use xai_grok_config::user_config::{set_user_default_model, user_config_path, user_default_model};

#[derive(Debug, Clone, Args)]
pub struct ModelsArgs {
    #[command(subcommand)]
    pub command: Option<ModelsCommand>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ModelsCommand {
    /// List configured models (default action when no subcommand is given)
    List {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
        /// Print one model reference per line.
        #[arg(long)]
        plain: bool,
    },
    /// Set the default model for new sessions
    Set {
        /// Model reference in `provider/model` form.
        model: String,
    },
}

pub fn run(args: ModelsArgs) -> Result<()> {
    match args.command.unwrap_or(ModelsCommand::List {
        json: false,
        plain: false,
    }) {
        ModelsCommand::List { json, plain } => list(json, plain),
        ModelsCommand::Set { model } => set(model),
    }
}

fn list(json: bool, plain: bool) -> Result<()> {
    let rows = read_entries()?;
    let default = user_default_model().context("failed to read default model")?;

    if plain {
        for r in &rows {
            println!("{}", r.ref_str);
        }
        return Ok(());
    }

    if json {
        let models: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "ref": r.ref_str,
                    "baseUrl": r.base_url,
                    "contextWindow": r.context_window,
                    "default": r.is_default,
                    "auth": r.auth,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "count": rows.len(),
                "default": default,
                "models": models,
            }))?
        );
        return Ok(());
    }

    if rows.is_empty() {
        println!("No models configured. Run `dttn configure` to add one.");
        return Ok(());
    }

    println!(
        "{:<34} {:<44} {:<9} {:<4} {}",
        "Model", "Base URL", "Ctx", "Def", "Auth"
    );
    for r in &rows {
        let ctx = r
            .context_window
            .map(humanize_ctx)
            .unwrap_or_else(|| "-".to_owned());
        println!(
            "{:<34} {:<44} {:<9} {:<4} {}",
            truncate(&r.ref_str, 34),
            truncate(&r.base_url, 44),
            ctx,
            if r.is_default { "*" } else { "" },
            r.auth,
        );
    }
    Ok(())
}

fn set(model: String) -> Result<()> {
    let rows = read_entries()?;
    if !rows.iter().any(|r| r.ref_str == model) {
        bail!(
            "model '{model}' is not configured. Run `dttn models list` to see configured \
             models, or `dttn configure` to add one."
        );
    }
    let path = set_user_default_model(&model).context("failed to set default model")?;
    println!("Default model set to {model}.");
    println!("Config file: {}", path.display());
    println!("Applies to new sessions; resumed sessions keep their frozen model.");
    Ok(())
}

struct ModelRow {
    ref_str: String,
    base_url: String,
    context_window: Option<i64>,
    is_default: bool,
    auth: &'static str,
}

fn read_entries() -> Result<Vec<ModelRow>> {
    let path = user_config_path();
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    let doc = raw.parse::<DocumentMut>().unwrap_or_default();
    let default = doc
        .get("models")
        .and_then(|m| m.as_table())
        .and_then(|t| t.get("default"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    let Some(table) = doc.get("model").and_then(|m| m.as_table()) else {
        return Ok(vec![]);
    };
    let mut rows: Vec<ModelRow> = table
        .iter()
        .filter_map(|(k, item)| {
            let entry = item.as_table()?;
            let ref_str = k.to_string();
            let base_url = entry
                .get("base_url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            let context_window = entry.get("context_window").and_then(|v| v.as_integer());
            let auth = if entry
                .get("api_key")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
            {
                "key"
            } else if entry.get("env_key").is_some() {
                "env"
            } else {
                "-"
            };
            let is_default = default.as_deref() == Some(ref_str.as_str());
            Some(ModelRow {
                ref_str,
                base_url,
                context_window,
                is_default,
                auth,
            })
        })
        .collect();
    rows.sort_by(|a, b| a.ref_str.cmp(&b.ref_str));
    Ok(rows)
}

fn humanize_ctx(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{}m", n / 1_000_000)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
