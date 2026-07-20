use std::fs;

use xai_grok_sampler::SamplerConfig;
use xai_grok_shell::session::session_model_snapshot::{
    ResolvedSessionModelSnapshot, SESSION_MODEL_SNAPSHOT_SCHEMA_VERSION, SessionModelSnapshotStore,
    apply_snapshot_to_sampler_config,
};

fn config(model: &str, context_window: u64, max_output: Option<u32>) -> SamplerConfig {
    SamplerConfig {
        model: model.to_string(),
        context_window,
        max_completion_tokens: max_output,
        ..Default::default()
    }
}

fn snapshot(
    model: &str,
    context_window: u64,
    max_output: Option<u32>,
    resolved_at_unix_ms: u64,
) -> ResolvedSessionModelSnapshot {
    let config = config(model, context_window, max_output);
    ResolvedSessionModelSnapshot {
        schema_version: SESSION_MODEL_SNAPSHOT_SCHEMA_VERSION,
        model_id: config.model.clone(),
        api_backend: config.api_backend,
        provider_extensions: config.provider_extensions,
        context_window,
        max_completion_tokens: max_output,
        catalog_metadata: None,
        catalog_origin: None,
        catalog_revision: None,
        catalog_fetched_at_unix_ms: None,
        catalog_expires_at_unix_ms: None,
        catalog_stale: false,
        resolved_at_unix_ms,
    }
}

#[test]
fn snapshot_round_trips_without_credentials_or_endpoint() {
    let temp = tempfile::tempdir().unwrap();
    let store = SessionModelSnapshotStore::new(temp.path());
    let snapshot = snapshot("company/model", 262_144, Some(65_536), 100);
    let path = store.store(&snapshot).unwrap();
    let raw = fs::read_to_string(path).unwrap();
    assert!(!raw.contains("api_key"));
    assert!(!raw.contains("base_url"));

    let restored = store.load_latest().unwrap().unwrap();
    assert_eq!(restored.model_id, "company/model");
    assert_eq!(restored.context_window, 262_144);
    assert_eq!(restored.max_completion_tokens, Some(65_536));
}

#[test]
fn corrupt_newest_entry_falls_back_to_older_valid_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let store = SessionModelSnapshotStore::new(temp.path());
    store
        .store(&snapshot("model-a", 128_000, Some(8_000), 100))
        .unwrap();
    fs::write(
        temp.path()
            .join("snapshot-99999999999999999999-corrupt.json"),
        b"{not-json",
    )
    .unwrap();

    let restored = store.load_latest().unwrap().unwrap();
    assert_eq!(restored.model_id, "model-a");
    assert_eq!(restored.context_window, 128_000);
}

#[test]
fn newest_explicit_model_switch_generation_wins() {
    let temp = tempfile::tempdir().unwrap();
    let store = SessionModelSnapshotStore::new(temp.path());
    store
        .store(&snapshot("model-a", 128_000, Some(8_000), 100))
        .unwrap();
    store
        .store(&snapshot("model-b", 256_000, Some(16_000), 200))
        .unwrap();

    let restored = store.load_latest().unwrap().unwrap();
    assert_eq!(restored.model_id, "model-b");
    assert_eq!(restored.context_window, 256_000);
}

#[test]
fn restored_snapshot_overrides_refreshed_runtime_limits() {
    let snapshot = snapshot("model-a", 128_000, Some(8_000), 100);
    let mut refreshed = config("model-a", 512_000, Some(64_000));
    assert!(apply_snapshot_to_sampler_config(&mut refreshed, &snapshot));
    assert_eq!(refreshed.context_window, 128_000);
    assert_eq!(refreshed.max_completion_tokens, Some(8_000));
}

#[test]
fn snapshot_for_another_model_is_rejected() {
    let snapshot = snapshot("model-a", 128_000, Some(8_000), 100);
    let mut config = config("model-b", 256_000, Some(16_000));
    assert!(!apply_snapshot_to_sampler_config(&mut config, &snapshot));
    assert_eq!(config.context_window, 256_000);
}
