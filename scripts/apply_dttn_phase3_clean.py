#!/usr/bin/env python3
"""Apply Phase 3 with literal detection that excludes function return types."""

from __future__ import annotations

import re
from pathlib import Path

import apply_dttn_phase3 as migration

ROOT = Path(__file__).resolve().parents[1]


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


def collapse_duplicates(text: str) -> str:
    pattern = re.compile(
        r"(?m)^(?P<indent>[ \t]*)provider_extensions: (?P<value>[^\n]+),\n"
        r"(?P=indent)provider_extensions: (?P=value),\n"
    )
    while pattern.search(text):
        text = pattern.sub(
            lambda match: (
                f"{match.group('indent')}provider_extensions: {match.group('value')},\n"
            ),
            text,
        )
    return text


def add_literal_fields(path: Path, type_name: str) -> None:
    text = collapse_duplicates(path.read_text(encoding="utf-8"))
    pattern = re.compile(rf"(?<![A-Za-z0-9_]){re.escape(type_name)}\s*\{{")
    edits: list[tuple[int, str]] = []
    for match in pattern.finditer(text):
        line_start = text.rfind("\n", 0, match.start()) + 1
        prefix = text[line_start : match.start()]
        if "->" in prefix or re.search(r"\b(struct|enum)\s+$", prefix):
            continue
        open_index = text.find("{", match.start(), match.end())
        close_index = matching_brace(text, open_index)
        block = text[open_index + 1 : close_index]
        if "provider_extensions:" in block:
            continue
        if not all(marker in block for marker in ("base_url:", "model:", "api_backend:")):
            continue
        api_match = re.search(
            r"(?m)^(?P<indent>[ \t]*)api_backend\s*:\s*[^\n]+,\s*$", block
        )
        if api_match is None:
            continue
        insert_at = open_index + 1 + api_match.end()
        edits.append(
            (
                insert_at,
                f"\n{api_match.group('indent')}provider_extensions: Default::default(),",
            )
        )
    for index, value in reversed(edits):
        text = text[:index] + value + text[index:]
    path.write_text(collapse_duplicates(text), encoding="utf-8")


def apply_literals() -> None:
    for path in sorted((ROOT / "crates" / "codegen").rglob("*.rs")):
        add_literal_fields(path, "SamplerConfig")
        add_literal_fields(path, "SamplingConfig")

    propagation = [
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
    for path, old, new in propagation:
        migration.replace_exact(path, old, new)

    for path in sorted((ROOT / "crates" / "codegen").rglob("*.rs")):
        path.write_text(collapse_duplicates(path.read_text(encoding="utf-8")), encoding="utf-8")


migration.matching_brace = matching_brace
migration.apply_sampling_types()
migration.apply_sampler_config()
migration.apply_model_config()
migration.apply_sampler_client()
migration.apply_model_defaults()
apply_literals()
print("DTTN Phase 3 provider compatibility migration applied")
