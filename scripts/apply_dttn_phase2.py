#!/usr/bin/env python3
"""Apply the deterministic DTTN Phase 2 runtime-identifier migration.

The script is intentionally strict: every source replacement must match exactly
once. A source drift aborts the migration instead of producing a partial edit.
"""

from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def replace_once(path: str, old: str, new: str) -> bool:
    target = ROOT / path
    text = target.read_text(encoding="utf-8")
    count = text.count(old)
    if count == 0 and new in text:
        return False
    if count != 1:
        raise RuntimeError(f"{path}: expected exactly one match, found {count}: {old!r}")
    target.write_text(text.replace(old, new, 1), encoding="utf-8")
    return True


def apply_config() -> None:
    path = "crates/codegen/xai-grok-shell/src/agent/config.rs"

    replacements = [
        (
            'pub const DEFAULT_AGENT_TYPE: &str = "grok-build-plan";',
            'pub const DEFAULT_AGENT_TYPE: &str = "dttn-code-agent";',
        ),
        (
            'pub const CLI_CHAT_PROXY_BASE_URL_DEFAULT: &str = "https://cli-chat-proxy.grok.com/v1";',
            'pub const CLI_CHAT_PROXY_BASE_URL_DEFAULT: &str = "https://gateway.dttn.invalid/v1";',
        ),
        (
            'pub const XAI_API_BASE_URL_DEFAULT: &str = "https://api.x.ai/v1";',
            'pub const XAI_API_BASE_URL_DEFAULT: &str = "https://inference.dttn.invalid/v1";',
        ),
        (
            'pub const ASSET_SERVER_URL_DEFAULT: &str = "https://assets.grok.com";',
            'pub const ASSET_SERVER_URL_DEFAULT: &str = "https://assets.dttn.invalid";',
        ),
        (
            '''pub(crate) fn default_asset_server_url() -> String {
    std::env::var("GROK_ASSET_SERVER_URL").unwrap_or_else(|_| ASSET_SERVER_URL_DEFAULT.to_owned())
}''',
            '''pub(crate) fn default_asset_server_url() -> String {
    env_string_with_legacy("DTTN_ASSET_SERVER_URL", "GROK_ASSET_SERVER_URL")
        .unwrap_or_else(|| ASSET_SERVER_URL_DEFAULT.to_owned())
}''',
        ),
        (
            '''        Self {
            cli_chat_proxy_base_url: std::env::var("GROK_CLI_CHAT_PROXY_BASE_URL").ok(),
            xai_api_base_url: std::env::var("GROK_XAI_API_BASE_URL")
                .unwrap_or_else(|_| XAI_API_BASE_URL_DEFAULT.to_owned()),
            alpha_test_key: None,
            models_base_url: env_string("GROK_MODELS_BASE_URL"),
            models_list_url: env_string("GROK_MODELS_LIST_URL"),
            feedback_base_url: env_string("GROK_FEEDBACK_BASE_URL"),
            trace_upload_url: env_string("GROK_TRACE_UPLOAD_URL"),
            trace_upload_bucket: env_string("GROK_TRACE_UPLOAD_BUCKET"),
            trace_upload_region: env_string("GROK_TRACE_UPLOAD_REGION"),
            trace_upload_credentials_file: env_string("GROK_TRACE_UPLOAD_CREDENTIALS_FILE"),
            trace_upload_credentials: None,
            trace_upload_endpoint_url: env_string("GROK_TRACE_UPLOAD_ENDPOINT_URL"),
            deployment_key: env_string("GROK_DEPLOYMENT_KEY"),
            managed_config_url: env_string("GROK_MANAGED_CONFIG_URL"),
            otel_exporter_otlp_endpoint: env_string("OTEL_EXPORTER_OTLP_ENDPOINT"),
            otel_exporter_otlp_traces_endpoint: env_string("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"),
            otel_exporter_otlp_headers: env_string("OTEL_EXPORTER_OTLP_HEADERS"),
            grok_internal_otlp_traces_endpoint: env_string("GROK_INTERNAL_OTLP_TRACES_ENDPOINT"),
            grok_internal_otlp_headers: env_string("GROK_INTERNAL_OTLP_HEADERS"),''',
            '''        Self {
            cli_chat_proxy_base_url: env_string_with_legacy(
                "DTTN_GATEWAY_BASE_URL",
                "GROK_CLI_CHAT_PROXY_BASE_URL",
            ),
            xai_api_base_url: env_string_with_legacy(
                "DTTN_INFERENCE_BASE_URL",
                "GROK_XAI_API_BASE_URL",
            )
            .unwrap_or_else(|| XAI_API_BASE_URL_DEFAULT.to_owned()),
            alpha_test_key: None,
            models_base_url: env_string_with_legacy(
                "DTTN_MODELS_BASE_URL",
                "GROK_MODELS_BASE_URL",
            ),
            models_list_url: env_string_with_legacy(
                "DTTN_MODELS_LIST_URL",
                "GROK_MODELS_LIST_URL",
            ),
            feedback_base_url: env_string_with_legacy(
                "DTTN_FEEDBACK_BASE_URL",
                "GROK_FEEDBACK_BASE_URL",
            ),
            trace_upload_url: env_string_with_legacy(
                "DTTN_TRACE_UPLOAD_URL",
                "GROK_TRACE_UPLOAD_URL",
            ),
            trace_upload_bucket: env_string_with_legacy(
                "DTTN_TRACE_UPLOAD_BUCKET",
                "GROK_TRACE_UPLOAD_BUCKET",
            ),
            trace_upload_region: env_string_with_legacy(
                "DTTN_TRACE_UPLOAD_REGION",
                "GROK_TRACE_UPLOAD_REGION",
            ),
            trace_upload_credentials_file: env_string_with_legacy(
                "DTTN_TRACE_UPLOAD_CREDENTIALS_FILE",
                "GROK_TRACE_UPLOAD_CREDENTIALS_FILE",
            ),
            trace_upload_credentials: None,
            trace_upload_endpoint_url: env_string_with_legacy(
                "DTTN_TRACE_UPLOAD_ENDPOINT_URL",
                "GROK_TRACE_UPLOAD_ENDPOINT_URL",
            ),
            deployment_key: env_string_with_legacy(
                "DTTN_DEPLOYMENT_KEY",
                "GROK_DEPLOYMENT_KEY",
            ),
            managed_config_url: env_string_with_legacy(
                "DTTN_MANAGED_CONFIG_URL",
                "GROK_MANAGED_CONFIG_URL",
            ),
            otel_exporter_otlp_endpoint: env_string("OTEL_EXPORTER_OTLP_ENDPOINT"),
            otel_exporter_otlp_traces_endpoint: env_string("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"),
            otel_exporter_otlp_headers: env_string("OTEL_EXPORTER_OTLP_HEADERS"),
            grok_internal_otlp_traces_endpoint: env_string_with_legacy(
                "DTTN_INTERNAL_OTLP_TRACES_ENDPOINT",
                "GROK_INTERNAL_OTLP_TRACES_ENDPOINT",
            ),
            grok_internal_otlp_headers: env_string_with_legacy(
                "DTTN_INTERNAL_OTLP_HEADERS",
                "GROK_INTERNAL_OTLP_HEADERS",
            ),''',
        ),
        (
            '''pub(crate) fn env_string(name: &str) -> Option<String> {
    let value = std::env::var(name).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
pub use xai_grok_config::env_bool;''',
            '''pub(crate) fn env_string(name: &str) -> Option<String> {
    let value = std::env::var(name).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Resolve a DTTN environment variable first, then a temporary legacy alias.
/// New deployments must only set the DTTN-prefixed variable.
pub(crate) fn env_string_with_legacy(primary: &str, legacy: &str) -> Option<String> {
    env_string(primary).or_else(|| env_string(legacy))
}
pub use xai_grok_config::env_bool;''',
        ),
    ]

    for old, new in replacements:
        replace_once(path, old, new)


def apply_main() -> None:
    path = "crates/codegen/xai-grok-pager-bin/src/main.rs"

    replacements = [
        ('eprintln!("   Grok agent server starting...");', 'eprintln!("   DTTN agent server starting...");'),
        ('/// Entrypoint tag for `grok -p`;', '/// Entrypoint tag for `dttn -p`;'),
        ('client_name: "grok-pager",', 'client_name: "dttn-cli",'),
        ('/// `grok setup`:', '/// `dttn setup`:'),
        ('using `grok login`', 'using `dttn login`'),
        ('export GROK_DEPLOYMENT_KEY=<your-key>', 'export DTTN_DEPLOYMENT_KEY=<your-key>'),
        ('$env:GROK_DEPLOYMENT_KEY=\\"<your-key>\\"', '$env:DTTN_DEPLOYMENT_KEY=\\"<your-key>\\"'),
        ('eprintln!("  grok setup");', 'eprintln!("  dttn setup");'),
        ('~/.grok/config.toml', '~/.dttn/config.toml'),
        ("contact your organization's Grok administrator", "contact your organization's DTTN administrator"),
        (
            "Your team doesn't have a managed configuration yet. A team admin can set one up at console.x.ai.",
            "Your team doesn't have a managed configuration yet. Contact your DTTN administrator.",
        ),
        ('PID {pid} is not a grok process', 'PID {pid} is not a DTTN process'),
        ('"grok-pager-leader-cli"', '"dttn-leader-cli"'),
        ('/// Env override for the `grok workspace` gate:', '/// Env override for the `dttn workspace` gate:'),
        ('/// Resolution of the `grok workspace` gate.', '/// Resolution of the `dttn workspace` gate.'),
        ('/// Truthy parse for grok on/off env vars:', '/// Truthy parse for DTTN on/off env vars:'),
        ('`grok workspace` is not enabled', '`dttn workspace` is not enabled'),
        ('settings for `grok workspace`', 'settings for `dttn workspace`'),
        ('run `grok login`', 'run `dttn login`'),
        ('"grok-workspace-cli"', '"dttn-workspace-cli"'),
        ('Start a grok session, or run `grok workspace start`.', 'Start a DTTN session, or run `dttn workspace start`.'),
        ('`grok workspace` requires leader mode', '`dttn workspace` requires leader mode'),
        ('No cached credentials found. Run `grok login` first.', 'No cached credentials found. Run `dttn login` first.'),
        (
            'const WORKSPACE_COMMAND_ENV: &str = "GROK_WORKSPACE_COMMAND";',
            'const WORKSPACE_COMMAND_ENV: &str = "DTTN_WORKSPACE_COMMAND";\nconst LEGACY_WORKSPACE_COMMAND_ENV: &str = "GROK_WORKSPACE_COMMAND";',
        ),
        (
            '''fn workspace_command_env_override() -> Option<bool> {
    std::env::var(WORKSPACE_COMMAND_ENV)
        .ok()
        .map(|v| env_flag_enabled(&v))
}''',
            '''fn workspace_command_env_override() -> Option<bool> {
    std::env::var(WORKSPACE_COMMAND_ENV)
        .or_else(|_| std::env::var(LEGACY_WORKSPACE_COMMAND_ENV))
        .ok()
        .map(|v| env_flag_enabled(&v))
}''',
        ),
    ]

    for old, new in replacements:
        replace_once(path, old, new)


if __name__ == "__main__":
    apply_config()
    apply_main()
    print("DTTN Phase 2 migration applied")
