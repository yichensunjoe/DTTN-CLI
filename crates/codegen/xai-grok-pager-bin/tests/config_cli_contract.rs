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
fn help_and_config_path_use_dttn_identity() {
    let home = temp_home();
    let help = run(&home, &["--help"]);
    assert!(help.status.success());
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("DTTN Agent CLI"));
    assert!(stdout.contains("config"));

    let path = run(&home, &["config", "path"]);
    assert!(path.status.success());
    assert_eq!(
        String::from_utf8_lossy(&path.stdout).trim(),
        home.join("config.toml").display().to_string()
    );
    let _ = std::fs::remove_dir_all(home);
}
