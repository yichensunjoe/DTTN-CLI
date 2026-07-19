use std::process::{Command, Output};

fn run_dttn(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dttn"))
        .args(args)
        .output()
        .expect("dttn binary should start")
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn model_refresh_help_is_successful_and_exposes_safe_credential_input() {
    let output = run_dttn(&["doctor", "model-refresh", "--help"]);
    let rendered = combined_output(&output);

    assert!(
        output.status.success(),
        "model-refresh help must exit successfully:\n{rendered}"
    );
    assert!(rendered.contains("model-refresh"));
    assert!(rendered.contains("--metadata-url"));
    assert!(rendered.contains("--token-env"));
    assert!(!rendered.contains("--token <TOKEN>"));
    assert!(!rendered.contains("--api-key"));
}

#[test]
fn inline_catalog_token_argument_is_rejected_without_echoing_its_value() {
    const SECRET_MARKER: &str = "DTTN_INLINE_SECRET_MUST_NOT_ECHO";
    let output = run_dttn(&[
        "doctor",
        "model-refresh",
        "--token",
        SECRET_MARKER,
    ]);
    let rendered = combined_output(&output);

    assert!(!output.status.success());
    assert!(rendered.contains("--token"));
    assert!(!rendered.contains(SECRET_MARKER));
}
