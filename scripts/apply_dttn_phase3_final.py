#!/usr/bin/env python3
"""Final Phase 3 migration runner with Rust-aware parsing and explicit propagation."""

from __future__ import annotations

from pathlib import Path

import apply_dttn_phase3 as migration


def matching_brace(text: str, open_index: int) -> int:
    depth = 0
    i = open_index
    state = "code"
    raw_hashes = 0
    while i < len(text):
        ch = text[i]
        nxt = text[i + 1] if i + 1 < len(text) else ""
        if state == "code":
            if ch == "r" and nxt in {'"', '#'}:
                j = i + 1
                hashes = 0
                while j < len(text) and text[j] == "#":
                    hashes += 1
                    j += 1
                if j < len(text) and text[j] == '"':
                    state = "raw_string"
                    raw_hashes = hashes
                    i = j
            elif ch == '"':
                state = "string"
            elif ch == "'":
                simple_char = i + 2 < len(text) and text[i + 2] == "'"
                escaped_char = i + 3 < len(text) and nxt == "\\" and text[i + 3] == "'"
                if simple_char or escaped_char:
                    state = "char"
            elif ch == "/" and nxt == "/":
                state = "line_comment"
                i += 1
            elif ch == "/" and nxt == "*":
                state = "block_comment"
                i += 1
            elif ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
                if depth == 0:
                    return i
        elif state == "string":
            if ch == "\\":
                i += 1
            elif ch == '"':
                state = "code"
        elif state == "raw_string":
            if ch == '"' and text.startswith("#" * raw_hashes, i + 1):
                i += raw_hashes
                state = "code"
        elif state == "char":
            if ch == "\\":
                i += 1
            elif ch == "'":
                state = "code"
        elif state == "line_comment":
            if ch == "\n":
                state = "code"
        elif state == "block_comment":
            if ch == "*" and nxt == "/":
                state = "code"
                i += 1
        i += 1
    raise RuntimeError(f"unbalanced Rust literal starting at byte {open_index}")


def apply_literal_updates() -> None:
    root = Path(__file__).resolve().parents[1]
    for path in sorted((root / "crates" / "codegen").rglob("*.rs")):
        migration.add_provider_field_to_literals(path, "SamplerConfig")
        migration.add_provider_field_to_literals(path, "SamplingConfig")

    replacements = [
        (
            "crates/codegen/xai-grok-shell/src/agent/config.rs",
            "        api_backend,\n        auth_scheme: credentials.auth_scheme,",
            "        api_backend,\n        provider_extensions: info.provider_extensions,\n        auth_scheme: credentials.auth_scheme,",
        ),
        (
            "crates/codegen/xai-grok-shell/src/agent/subagent/mod.rs",
            "                api_backend: cfg.api_backend,\n                provider_extensions: Default::default(),",
            "                api_backend: cfg.api_backend,\n                provider_extensions: cfg.provider_extensions,",
        ),
        (
            "crates/codegen/xai-grok-shell/src/session/acp_session_impl/sampler_turn.rs",
            "            api_backend: cfg.api_backend,\n            provider_extensions: Default::default(),",
            "            api_backend: cfg.api_backend,\n            provider_extensions: cfg.provider_extensions,",
        ),
        (
            "crates/codegen/xai-grok-shell/src/session/acp_session_impl/model_switch.rs",
            "                api_backend: sampling_config.api_backend.clone(),\n                provider_extensions: Default::default(),",
            "                api_backend: sampling_config.api_backend.clone(),\n                provider_extensions: sampling_config.provider_extensions,",
        ),
        (
            "crates/codegen/xai-grok-shell/src/session/acp_session_impl/spawn.rs",
            "        api_backend: sampling_config.api_backend.clone(),\n        provider_extensions: Default::default(),",
            "        api_backend: sampling_config.api_backend.clone(),\n        provider_extensions: sampling_config.provider_extensions,",
        ),
    ]
    for path, old, new in replacements:
        migration.replace_exact(path, old, new)


migration.matching_brace = matching_brace
migration.apply_sampling_types()
migration.apply_sampler_config()
migration.apply_model_config()
migration.apply_sampler_client()
migration.apply_model_defaults()
apply_literal_updates()
print("DTTN Phase 3 provider compatibility migration applied")
