#!/usr/bin/env python3
"""Idempotently wire and normalize `dttn doctor model`."""

from pathlib import Path

main_path = Path("crates/codegen/xai-grok-pager-bin/src/main.rs")
main_text = main_path.read_text(encoding="utf-8")
marker = "xai_grok_pager::model_doctor::try_run_from_env().await?"

if marker not in main_text:
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
    count = main_text.count(needle)
    if count != 1:
        raise SystemExit(f"expected one async_main insertion point, found {count}")
    main_path.write_text(main_text.replace(needle, replacement, 1), encoding="utf-8")
    print("wired model doctor entrypoint")
else:
    print("model doctor entrypoint already present")

doctor_path = Path("crates/codegen/xai-grok-pager/src/model_doctor.rs")
doctor_text = doctor_path.read_text(encoding="utf-8")
old = """    let agent_config = AgentConfig::new_from_toml_cfg(&raw)
        .context(\"failed to parse effective DTTN configuration\")?;
"""
new = """    let agent_config = AgentConfig::new_from_toml_cfg(&raw)
        .map_err(|error| anyhow!(\"failed to parse effective DTTN configuration: {error}\"))?;
"""
if old in doctor_text:
    doctor_path.write_text(doctor_text.replace(old, new, 1), encoding="utf-8")
    print("normalized model doctor config error mapping")
elif new not in doctor_text:
    raise SystemExit("model doctor config error mapping did not match expected source")
else:
    print("model doctor config error mapping already normalized")
