//! Interactive model-configuration wizard (OpenClaw-style) using `cliclack`.
//!
//! Mirrors the OpenClaw `config` -> `model` -> custom-api-key flow: pick a
//! provider (or "Custom"), fill in base URL + API key + model id, save as a new
//! `[model."provider/model"]` entry without disturbing existing entries.
//!
//! All prompts are gated on an interactive TTY. Headless / non-TTY callers get a
//! clean error pointing at the non-interactive `dttn config models custom` path;
//! `dttn --single` never reaches this code (the launch gate is skipped when
//! `is_interactive` is false).

use std::io::IsTerminal;

use anyhow::{Context, Result, bail};
use cliclack::{confirm, input, intro, log, note, outro, password, select};
use toml_edit::DocumentMut;

use xai_grok_config::model_providers::{ModelProviderDescriptor, model_providers};
use xai_grok_config::user_config::{
    CustomModelApiBackend, CustomModelAuthScheme, CustomModelConfig, set_custom_model,
    set_user_default_model, user_config_path, user_default_model,
};

/// Sentinel provider id offered in the picker to bail out without starting the TUI.
const QUIT_SENTINEL: &str = "__quit__";

/// What `dttn configure` / bare `dttn config` launches.
pub fn run_wizard() -> Result<()> {
    if !std::io::stdin().is_terminal() {
        bail!(
            "dttn configure requires an interactive terminal.\n\
             To configure non-interactively, use:\n  \
             dttn config models custom <PROVIDER> <MODEL> --base-url <URL> \
             --api-key-env <ENV> --context-window <TOKENS> [--set-default]"
        );
    }
    intro("dttn configure")?;
    loop {
        let configured = list_configured_models();
        if configured.refs.is_empty() {
            note("Welcome", "No models configured yet. Let's add one.")?;
        } else {
            let mut summary = String::new();
            for r in &configured.refs {
                let marker = if configured.default.as_deref() == Some(r.as_str()) {
                    " (default)"
                } else {
                    ""
                };
                summary.push_str(&format!("  - {r}{marker}\n"));
            }
            note("Current configuration", summary.trim_end())?;
        }

        let action: &str = select("What do you want to do?")
            .item("model", "Add / configure a model", "provider + credentials")
            .item("done", "Done", "exit the wizard")
            .interact()?;
        if action == "done" {
            break;
        }

        let is_first = configured.refs.is_empty();
        match prompt_one_model(is_first) {
            Ok(ref_str) => log::success(format!("Saved {ref_str}."))?,
            Err(err) => note("Skipped - not saved", format!("{err:#}"))?,
        }

        if !confirm("Configure another model?").interact()? {
            break;
        }
    }
    outro("Configuration saved.")?;
    Ok(())
}

/// Launch-time gate: ensure a usable default model exists before entering the TUI.
///
/// Returns `true` to proceed to the TUI, `false` to exit without starting it
/// (user declined to pick / configure). Only invoked on the bare-interactive
/// launch path; headless and subcommand paths never reach it.
pub fn ensure_model_for_launch() -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        // No way to prompt - defer to the TUI/welcome flow.
        return Ok(true);
    }
    if user_default_model()?.is_some() {
        return Ok(true);
    }
    let refs = list_configured_models().refs;
    if refs.is_empty() {
        // Zero models -> run the wizard. It sets the first model as default.
        run_wizard()?;
        return Ok(user_default_model()?.is_some());
    }
    // Models exist but no default -> quick picker.
    let mut picker = select::<String>("Choose a model to use for this session");
    for r in &refs {
        picker = picker.item(r.clone(), r.clone(), "");
    }
    picker = picker.item(
        QUIT_SENTINEL.to_owned(),
        "Quit",
        "exit without starting the TUI",
    );
    let choice: String = picker.interact()?;
    if choice == QUIT_SENTINEL {
        return Ok(false);
    }
    set_user_default_model(&choice).context("failed to set default model")?;
    Ok(true)
}

/// Configured `[model.*]` refs and the current default, read from the user
/// config file only (no network, no managed/requirements merge).
#[derive(Debug, Default, Clone)]
pub struct ConfiguredModels {
    pub refs: Vec<String>,
    pub default: Option<String>,
}

pub fn list_configured_models() -> ConfiguredModels {
    let path = user_config_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return ConfiguredModels::default();
    };
    let Ok(doc) = raw.parse::<DocumentMut>() else {
        return ConfiguredModels::default();
    };
    let default = doc
        .get("models")
        .and_then(|m| m.as_table())
        .and_then(|t| t.get("default"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let refs = doc
        .get("model")
        .and_then(|m| m.as_table())
        .map(|t| t.iter().map(|(k, _)| k.to_string()).collect())
        .unwrap_or_default();
    ConfiguredModels { refs, default }
}

/// Collect one model's settings via prompts and persist it.
fn prompt_one_model(is_first: bool) -> Result<String> {
    let providers = model_providers();

    // 1. Provider (curated, where we can map to a supported backend) + Custom.
    let mut sel = select("Choose a model provider");
    let mut chosen: Option<&'static ModelProviderDescriptor> = None;
    for p in providers {
        if p.id == "custom" {
            continue;
        }
        if backend_for_style(p.api_style).is_some() {
            sel = sel.item(p.id, p.name, p.description);
        }
    }
    sel = sel.item(
        "custom",
        "Custom API key",
        "any OpenAI/Anthropic-compatible endpoint",
    );
    let provider_choice: &str = sel.interact()?;
    if provider_choice != "custom" {
        chosen = providers.iter().find(|p| p.id == provider_choice);
    }

    // 2. Base URL (prefilled from the curated provider when available).
    let preset_base = chosen.and_then(|p| p.default_base_url);
    let mut base_url_prompt = input("API Base URL").placeholder("https://api.example.com/v1");
    if let Some(preset) = preset_base {
        base_url_prompt = base_url_prompt.default_input(preset);
    }
    let base_url: String = base_url_prompt
        .validate(|s: &String| {
            let t = s.trim();
            if t.is_empty() {
                Err("Base URL is required".to_string())
            } else if !(t.starts_with("http://") || t.starts_with("https://")) {
                Err("Must start with http:// or https://".to_string())
            } else {
                Ok(())
            }
        })
        .interact()?;

    // 3. Endpoint compatibility (only for Custom; curated uses its preset).
    let backend = if let Some(p) = chosen {
        backend_for_style(p.api_style).unwrap_or(CustomModelApiBackend::ChatCompletions)
    } else {
        let compat: &str = select("Endpoint compatibility")
            .item("chat", "OpenAI Chat Completions", "most providers")
            .item("responses", "OpenAI Responses", "")
            .item("messages", "Anthropic Messages", "")
            .interact()?;
        match compat {
            "responses" => CustomModelApiBackend::Responses,
            "messages" => CustomModelApiBackend::Messages,
            _ => CustomModelApiBackend::ChatCompletions,
        }
    };
    let auth_scheme = match backend {
        CustomModelApiBackend::Messages => CustomModelAuthScheme::XApiKey,
        _ => CustomModelAuthScheme::Bearer,
    };

    // 4. Model ID.
    let model_id: String = input("Model ID")
        .placeholder("e.g. kimi-for-coding, gpt-4o, llama3")
        .validate(|s: &String| {
            if s.trim().is_empty() {
                Err("Model ID is required".to_string())
            } else {
                Ok(())
            }
        })
        .interact()?;

    // 5. Auth method & API key (OAuth or plaintext API key).
    let provider_is_moonshot = chosen
        .map(|p| p.id == "moonshot" || p.id == "kimi")
        .unwrap_or_else(|| {
            let lower_url = base_url.to_lowercase();
            let lower_model = model_id.to_lowercase();
            lower_url.contains("moonshot") || lower_url.contains("kimi") || lower_model.contains("kimi")
        });

    let auth_mode: &str = if provider_is_moonshot {
        select("Authentication method")
            .item("oauth", "OAuth 2.0 (Login via Kimi Browser / Device Flow)", "Recommended - login with Kimi account")
            .item("apikey", "API key (sk-...)", "Manual API key entry")
            .interact()?
    } else {
        "apikey"
    };

    let api_key: String = if auth_mode == "oauth" {
        perform_kimi_oauth_flow()?
    } else {
        password("API key")
            .mask('•')
            .validate(|s: &String| {
                if s.trim().is_empty() {
                    Err("API key is required".to_string())
                } else {
                    Ok(())
                }
            })
            .interact()?
    };

    // 6. Context window.
    let context_window: u64 = input("Context window (tokens)")
        .default_input("128000")
        .validate(|s: &String| {
            let v: u64 = s
                .parse()
                .map_err(|_| "must be a positive integer".to_string())?;
            if v == 0 {
                return Err("must be greater than zero".to_string());
            }
            Ok(())
        })
        .interact()?;

    // 7. Display name (optional).
    let name: String = input("Display name (optional)")
        .placeholder("e.g. Kimi for Coding")
        .interact()?;
    let display_name = name.trim().is_empty().then(|| name.trim().to_owned());

    // 8. Provider id (curated uses its id; Custom derives one from the host).
    let provider_id = match chosen {
        Some(p) => p.id.to_owned(),
        None => {
            let suggested = derive_provider_id(&base_url);
            let raw: String = input("Provider id")
                .default_input(&suggested)
                .interact()?;
            let raw = raw.trim();
            if raw.is_empty() {
                suggested
            } else {
                raw.to_owned()
            }
        }
    };

    // 9. Default? The first model auto-becomes the default; otherwise ask.
    let set_default = is_first || confirm("Make this the default model for new sessions?").interact()?;

    let config = CustomModelConfig {
        provider_id,
        model_id,
        display_name,
        base_url,
        api_key: Some(api_key),
        api_key_env: None,
        api_backend: backend,
        auth_scheme,
        context_window,
        max_completion_tokens: None,
        set_default,
    };
    let result = set_custom_model(&config).context("failed to save model")?;
    Ok(result.model_ref)
}

/// Map a curated provider's `api_style` to a DTTN-supported custom backend.
/// Returns `None` for provider-native protocols (Gemini, Ollama) that the
/// custom-model command does not configure; those are not offered as curated
/// picks (use "Custom" instead).
fn backend_for_style(api_style: &str) -> Option<CustomModelApiBackend> {
    match api_style {
        "openai" | "openai-compatible" => Some(CustomModelApiBackend::ChatCompletions),
        "anthropic-messages" => Some(CustomModelApiBackend::Messages),
        _ => None,
    }
}

/// Derive a valid provider id from a base URL's host (for the Custom path).
fn derive_provider_id(base_url: &str) -> String {
    let host = base_url
        .split("://")
        .nth(1)
        .unwrap_or(base_url)
        .split('/')
        .next()
        .unwrap_or("");
    let mut s: String = host
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s = s.trim_matches('-').to_owned();
    if s.is_empty() {
        s = "custom".to_owned();
    }
    let first = s.chars().next().unwrap();
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        s = format!("custom-{s}");
    }
    s
}

fn block_on_async<F: std::future::Future>(future: F) -> F::Output {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(future))
    } else {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to initialize async runtime for OAuth");
        rt.block_on(future)
    }
}

pub fn perform_kimi_oauth_flow() -> Result<String> {
    use xai_grok_config::provider_oauth::{save_oauth_credential, KimiOAuthClient};

    let device_auth = block_on_async(KimiOAuthClient::request_device_authorization())
        .map_err(|e| anyhow::anyhow!("Kimi OAuth authorization request failed: {e}"))?;

    note(
        "Kimi OAuth Authorization",
        format!(
            "Please open this URL in your browser:\n  {}\n\nVerification Code: {}\n",
            device_auth.verification_uri_complete, device_auth.user_code
        ),
    )?;

    let _ = webbrowser::open(&device_auth.verification_uri_complete);

    let spinner = cliclack::spinner();
    spinner.start("Waiting for Kimi login authorization in browser...");

    let cred_result = block_on_async(KimiOAuthClient::poll_for_token(
        &device_auth.device_code,
        device_auth.interval_ms,
        device_auth.expires_in_ms,
        || {},
    ));

    match cred_result {
        Ok(cred) => {
            spinner.stop("Authorization successful!");
            save_oauth_credential("kimi", &cred)
                .map_err(|e| anyhow::anyhow!("Failed to save credential: {e}"))?;
            save_oauth_credential("moonshot", &cred)
                .map_err(|e| anyhow::anyhow!("Failed to save credential: {e}"))?;
            Ok(cred.access)
        }
        Err(e) => {
            spinner.stop("Authorization failed or timed out.");
            bail!("Kimi OAuth error: {}", e);
        }
    }
}

