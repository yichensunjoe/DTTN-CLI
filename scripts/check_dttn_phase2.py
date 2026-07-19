#!/usr/bin/env python3
"""Validate DTTN Phase 2 runtime identifiers and endpoint defaults."""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def main() -> int:
    errors: list[str] = []

    config_path = "crates/codegen/xai-grok-shell/src/agent/config.rs"
    config = read(config_path)
    required_config = (
        'pub const DEFAULT_AGENT_TYPE: &str = "dttn-code-agent";',
        'pub const CLI_CHAT_PROXY_BASE_URL_DEFAULT: &str = "https://gateway.dttn.invalid/v1";',
        'pub const XAI_API_BASE_URL_DEFAULT: &str = "https://inference.dttn.invalid/v1";',
        'pub const ASSET_SERVER_URL_DEFAULT: &str = "https://assets.dttn.invalid";',
        '"DTTN_GATEWAY_BASE_URL"',
        '"DTTN_INFERENCE_BASE_URL"',
        '"DTTN_MODELS_BASE_URL"',
        '"DTTN_MODELS_LIST_URL"',
        '"DTTN_DEPLOYMENT_KEY"',
        '"DTTN_MANAGED_CONFIG_URL"',
        '"DTTN_INTERNAL_OTLP_TRACES_ENDPOINT"',
    )
    for marker in required_config:
        if marker not in config:
            errors.append(f"{config_path}: missing {marker}")

    forbidden_default_assignments = (
        'pub const CLI_CHAT_PROXY_BASE_URL_DEFAULT: &str = "https://cli-chat-proxy.grok.com/v1";',
        'pub const XAI_API_BASE_URL_DEFAULT: &str = "https://api.x.ai/v1";',
        'pub const ASSET_SERVER_URL_DEFAULT: &str = "https://assets.grok.com";',
        'pub const DEFAULT_AGENT_TYPE: &str = "grok-build-plan";',
    )
    for marker in forbidden_default_assignments:
        if marker in config:
            errors.append(f"{config_path}: forbidden runtime default assignment {marker!r}")

    main_path = "crates/codegen/xai-grok-pager-bin/src/main.rs"
    main_rs = read(main_path)
    required_main = (
        "DTTN agent server starting",
        'client_name: "dttn-cli"',
        '"dttn-leader-cli"',
        '"dttn-workspace-cli"',
        '"DTTN_WORKSPACE_COMMAND"',
        "~/.dttn/config.toml",
        "`dttn login`",
        "`dttn workspace`",
    )
    for marker in required_main:
        if marker not in main_rs:
            errors.append(f"{main_path}: missing {marker}")

    forbidden_main = (
        "Grok agent server starting",
        'client_name: "grok-pager"',
        '"grok-pager-leader-cli"',
        '"grok-workspace-cli"',
        "console.x.ai",
        "`grok login`",
        "`grok workspace`",
        "~/.grok/config.toml",
    )
    for marker in forbidden_main:
        if marker in main_rs:
            errors.append(f"{main_path}: forbidden user-facing marker {marker!r}")

    if errors:
        print("DTTN Phase 2 validation failed:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1

    print("DTTN Phase 2 validation passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
