use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_home() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("dttn-config-cli-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn run(home: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dttn"))
        .env("DTTN_HOME", home)
        .env_remove("GROK_HOME")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn model_command_updates_and_resets_config_without_starting_agent() {
    let home = temp_home();
    let set = run(&home, &["config", "model", "provider/model-v1", "--json"]);
    assert!(
        set.status.success(),
        "{}",
        String::from_utf8_lossy(&set.stderr)
    );
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(config.contains("default = \"provider/model-v1\""));

    let show = run(&home, &["config", "model", "--json"]);
    assert!(
        show.status.success(),
        "{}",
        String::from_utf8_lossy(&show.stderr)
    );
    let payload: serde_json::Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(payload["userDefault"], "provider/model-v1");
    assert_eq!(payload["effectiveDefault"], "provider/model-v1");
    assert_eq!(payload["appliesTo"], "new_sessions");

    let summary = run(&home, &["config"]);
    assert!(
        summary.status.success(),
        "{}",
        String::from_utf8_lossy(&summary.stderr)
    );
    let summary_stdout = String::from_utf8_lossy(&summary.stdout);
    assert!(summary_stdout.contains("User default:      provider/model-v1"));
    assert!(summary_stdout.contains("Effective default: provider/model-v1"));

    let reset = run(&home, &["config", "model", "--reset", "--json"]);
    assert!(
        reset.status.success(),
        "{}",
        String::from_utf8_lossy(&reset.stderr)
    );
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(!config.contains("default ="));
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn config_models_lists_only_the_curated_provider_directory() {
    let home = temp_home();
    let output = run(&home, &["config", "models", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let ids: Vec<_> = payload["providers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|provider| provider["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "deepseek",
            "google",
            "lmstudio",
            "minimax",
            "moonshot",
            "ollama",
            "ollama-cloud",
            "openai",
            "opencode",
            "opencode-go",
            "openrouter",
            "qwen",
            "stepfun",
            "xiaomi",
            "zai",
            "custom",
        ]
    );
    assert_eq!(payload["modelRefFormat"], "provider/model");
    assert_eq!(
        payload["customProvider"]["supportedAuthSchemes"],
        serde_json::json!(["bearer", "x_api_key"])
    );
    assert_eq!(
        payload["customProvider"]["changesDefaultOnlyWith"],
        "--set-default"
    );
    assert!(!home.join("config.toml").exists());
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn custom_provider_model_is_persisted_without_plaintext_credentials() {
    let home = temp_home();
    let output = run(
        &home,
        &[
            "config",
            "models",
            "custom",
            "acme",
            "code-v1",
            "--name",
            "Acme Code",
            "--base-url",
            "https://models.acme.test/v1",
            "--api-key-env",
            "ACME_API_KEY",
            "--backend",
            "messages",
            "--context-window",
            "131072",
            "--max-completion-tokens",
            "8192",
            "--set-default",
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["modelRef"], "acme/code-v1");
    assert_eq!(payload["defaultChanged"], true);
    assert_eq!(payload["appliesTo"], "new_sessions");

    let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(config.contains("default = \"acme/code-v1\""));
    assert!(config.contains("[model.\"acme/code-v1\"]"));
    assert!(config.contains("model = \"code-v1\""));
    assert!(config.contains("base_url = \"https://models.acme.test/v1\""));
    assert!(config.contains("env_key = \"ACME_API_KEY\""));
    assert!(config.contains("api_backend = \"messages\""));
    assert!(config.contains("auth_scheme = \"x_api_key\""));
    assert!(config.contains("context_window = 131072"));
    assert!(config.contains("max_completion_tokens = 8192"));
    assert!(!config.contains("sk-"));
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn custom_provider_does_not_change_default_without_explicit_flag() {
    let home = temp_home();
    let initial = run(&home, &["config", "model", "existing/model", "--json"]);
    assert!(initial.status.success());
    let custom = run(
        &home,
        &[
            "config",
            "models",
            "custom",
            "local",
            "model-v2",
            "--base-url",
            "http://localhost:1234/v1",
            "--context-window",
            "65536",
            "--json",
        ],
    );
    assert!(
        custom.status.success(),
        "{}",
        String::from_utf8_lossy(&custom.stderr)
    );
    let payload: serde_json::Value = serde_json::from_slice(&custom.stdout).unwrap();
    assert_eq!(payload["defaultChanged"], false);
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(config.contains("default = \"existing/model\""));
    assert!(config.contains("[model.\"local/model-v2\"]"));
    assert!(!config.contains("env_key ="));
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn help_and_config_path_use_dttn_identity() {
    let home = temp_home();
    let help = run(&home, &["--help"]);
    assert!(help.status.success());
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("DTTN Agent CLI"));
    assert!(stdout.contains("config"));

    let config_help = run(&home, &["config", "--help"]);
    assert!(config_help.status.success());
    assert!(String::from_utf8_lossy(&config_help.stdout).contains("models"));

    let path = run(&home, &["config", "path"]);
    assert!(path.status.success());
    assert_eq!(
        String::from_utf8_lossy(&path.stdout).trim(),
        home.join("config.toml").display().to_string()
    );
    let _ = std::fs::remove_dir_all(home);
}
