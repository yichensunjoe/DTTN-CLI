#!/usr/bin/env python3
"""Validate the Phase 1 DTTN distribution boundary.

This check intentionally targets user-facing and distribution-default files.
Internal compatibility identifiers are removed in later migration phases.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXPECTED_MODEL = "agnes-2.0-flash"


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def fail(errors: list[str], message: str) -> None:
    errors.append(message)


def main() -> int:
    errors: list[str] = []

    model_path = "crates/codegen/xai-grok-models/default_models.json"
    model_text = read(model_path)
    try:
        model_config = json.loads(model_text)
    except json.JSONDecodeError as exc:
        fail(errors, f"{model_path}: invalid JSON: {exc}")
        model_config = {}

    for role in ("default", "web_search", "image_description", "session_summary"):
        if model_config.get(role) != EXPECTED_MODEL:
            fail(
                errors,
                f"{model_path}: {role} must be {EXPECTED_MODEL!r}, got {model_config.get(role)!r}",
            )

    catalog_ids = {
        entry.get("model")
        for entry in model_config.get("models", [])
        if isinstance(entry, dict)
    }
    if EXPECTED_MODEL not in catalog_ids:
        fail(errors, f"{model_path}: default model is missing from the embedded catalog")

    if "grok" in model_text.casefold():
        fail(errors, f"{model_path}: legacy vendor branding is present")

    for path in ("README.md", "CONTRIBUTING.md", "SECURITY.md"):
        text = read(path).casefold()
        for forbidden in ("grok", "spacexai", "console.x.ai", "api.x.ai"):
            if forbidden in text:
                fail(errors, f"{path}: forbidden user-facing identifier {forbidden!r}")

    binary_manifest = read("crates/codegen/xai-grok-pager-bin/Cargo.toml")
    if 'default-run = "dttn"' not in binary_manifest:
        fail(errors, "binary manifest: default-run must be dttn")
    if '[[bin]]\nname = "dttn"' not in binary_manifest:
        fail(errors, "binary manifest: shipped binary must be named dttn")

    paths_rs = read("crates/codegen/xai-grok-config/src/paths.rs")
    for required in ('"DTTN_HOME"', 'join(".dttn")', 'PathBuf::from("/etc/dttn")'):
        if required not in paths_rs:
            fail(errors, f"config paths: missing required DTTN path marker {required}")

    if errors:
        print("DTTN Phase 1 validation failed:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1

    print("DTTN Phase 1 validation passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
