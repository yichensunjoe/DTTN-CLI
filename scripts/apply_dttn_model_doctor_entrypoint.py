#!/usr/bin/env python3
"""Idempotently wire `dttn doctor model` into the async composition root."""

from pathlib import Path

path = Path("crates/codegen/xai-grok-pager-bin/src/main.rs")
text = path.read_text(encoding="utf-8")
marker = "xai_grok_pager::model_doctor::try_run_from_env().await?"

if marker in text:
    print("model doctor entrypoint already present")
    raise SystemExit(0)

needle = """async fn async_main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut args = PagerArgs::parse_and_apply_cwd()?;
"""
replacement = """async fn async_main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    if xai_grok_pager::model_doctor::try_run_from_env().await? {
        return Ok(());
    }
    let mut args = PagerArgs::parse_and_apply_cwd()?;
"""

count = text.count(needle)
if count != 1:
    raise SystemExit(f"expected one async_main insertion point, found {count}")

path.write_text(text.replace(needle, replacement, 1), encoding="utf-8")
print("wired model doctor entrypoint")
