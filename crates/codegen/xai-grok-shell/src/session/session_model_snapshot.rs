//! Session-frozen model metadata.
//!
//! A session resolves its runtime model contract once and stores a
//! credential-free snapshot beside the session. Resume restores the
//! newest valid snapshot instead of silently adopting refreshed
//! provider metadata. Explicit model switches append a new snapshot.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use xai_grok_sampler::SamplerConfig;
use xai_grok_sampling_types::{ApiBackend, ModelMetadata, ProviderExtensions};

use crate::model_catalog_runtime::{CatalogFreshness, default_model_catalog_cache};
use crate::session::info::Info as SessionInfo;

pub const SESSION_MODEL_SNAPSHOT_SCHEMA_VERSION: u32 = 1;
const SNAPSHOT_DIRECTORY: &str = "resolved-model-v1";
const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
const DEFAULT_SNAPSHOT_ENTRIES: usize = 4;
static SNAPSHOT_NONCE: AtomicU64 = AtomicU64::new(0);

/// Credential-free model contract frozen for one session generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedSessionModelSnapshot {
    pub schema_version: u32,
    pub model_id: String,
    pub api_backend: ApiBackend,
    pub provider_extensions: ProviderExtensions,
    pub context_window: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_metadata: Option<ModelMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_fetched_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_expires_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub catalog_stale: bool,
    pub resolved_at_unix_ms: u64,
}

impl ResolvedSessionModelSnapshot {
    fn validate(&self) -> Result<(), SessionModelSnapshotError> {
        if self.schema_version != SESSION_MODEL_SNAPSHOT_SCHEMA_VERSION {
            return Err(SessionModelSnapshotError::Invalid(format!(
                "unsupported schema version {}",
                self.schema_version
            )));
        }
        if self.model_id.trim().is_empty() {
            return Err(SessionModelSnapshotError::Invalid(
                "model id must not be blank".to_string(),
            ));
        }
        if self.context_window == 0 {
            return Err(SessionModelSnapshotError::Invalid(
                "context window must be greater than zero".to_string(),
            ));
        }
        if self
            .catalog_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.model_id != self.model_id)
        {
            return Err(SessionModelSnapshotError::Invalid(
                "catalog metadata model id does not match runtime model".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotResolutionKind {
    Restored,
    Created,
}

#[derive(Debug, Clone)]
pub struct SnapshotResolution {
    pub snapshot: ResolvedSessionModelSnapshot,
    pub kind: SnapshotResolutionKind,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionModelSnapshotError {
    #[error("session model snapshot I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("session model snapshot serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("session model snapshot is invalid: {0}")]
    Invalid(String),
}

/// Append-only, atomic snapshot store. Readers skip corrupt or oversized
/// entries and fall back to the newest older valid snapshot.
#[derive(Debug, Clone)]
pub struct SessionModelSnapshotStore {
    directory: PathBuf,
    max_entries: usize,
}

impl SessionModelSnapshotStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            max_entries: DEFAULT_SNAPSHOT_ENTRIES,
        }
    }

    #[cfg(test)]
    fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries.max(1);
        self
    }

    pub fn load_latest(
        &self,
    ) -> Result<Option<ResolvedSessionModelSnapshot>, SessionModelSnapshotError> {
        let mut paths = match fs::read_dir(&self.directory) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| is_snapshot_path(path))
                .collect::<Vec<_>>(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        paths.sort_unstable_by(|left, right| right.file_name().cmp(&left.file_name()));

        for path in paths {
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => continue,
                Ok(metadata) if metadata.len() as usize > MAX_SNAPSHOT_BYTES => continue,
                Ok(_) => {}
                Err(_) => continue,
            }
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            let snapshot = match serde_json::from_slice::<ResolvedSessionModelSnapshot>(&bytes) {
                Ok(snapshot) => snapshot,
                Err(_) => continue,
            };
            if snapshot.validate().is_ok() {
                return Ok(Some(snapshot));
            }
        }
        Ok(None)
    }

    pub fn store(
        &self,
        snapshot: &ResolvedSessionModelSnapshot,
    ) -> Result<PathBuf, SessionModelSnapshotError> {
        snapshot.validate()?;
        let json = serde_json::to_vec_pretty(snapshot)?;
        if json.len() > MAX_SNAPSHOT_BYTES {
            return Err(SessionModelSnapshotError::Invalid(format!(
                "serialized snapshot exceeds {MAX_SNAPSHOT_BYTES} bytes"
            )));
        }

        fs::create_dir_all(&self.directory)?;
        let nonce = SNAPSHOT_NONCE.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let final_name = format!(
            "snapshot-{:020}-{pid:010}-{nonce:020}.json",
            snapshot.resolved_at_unix_ms
        );
        let temp_path = self.directory.join(format!(".{final_name}.tmp"));
        let final_path = self.directory.join(final_name);

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp_path)?;
        file.write_all(&json)?;
        file.sync_all()?;
        drop(file);
        if let Err(error) = fs::rename(&temp_path, &final_path) {
            let _ = fs::remove_file(&temp_path);
            return Err(error.into());
        }
        self.prune_old_entries();
        Ok(final_path)
    }

    fn prune_old_entries(&self) {
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return;
        };
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| is_snapshot_path(path))
            .collect::<Vec<_>>();
        paths.sort_unstable_by(|left, right| right.file_name().cmp(&left.file_name()));
        for path in paths.into_iter().skip(self.max_entries) {
            let _ = fs::remove_file(path);
        }
    }
}

/// Restore the newest snapshot when it belongs to the active model;
/// otherwise create a new generation from the final runtime config.
pub fn resolve_or_create_session_snapshot(
    session_info: &SessionInfo,
    config: &SamplerConfig,
    effective_context_window: u64,
) -> SnapshotResolution {
    let store = session_store(session_info);
    match store.load_latest() {
        Ok(Some(snapshot)) if snapshot.model_id == config.model => {
            return SnapshotResolution {
                snapshot,
                kind: SnapshotResolutionKind::Restored,
            };
        }
        Ok(Some(snapshot)) => {
            tracing::info!(
                session_id = %session_info.id.0,
                persisted_model = %snapshot.model_id,
                active_model = %config.model,
                "session model changed; creating a new frozen model snapshot"
            );
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(
                session_id = %session_info.id.0,
                error = %error,
                "failed to load session model snapshot; rebuilding from runtime config"
            );
        }
    }

    let snapshot = build_snapshot(config, effective_context_window);
    if let Err(error) = store.store(&snapshot) {
        tracing::warn!(
            session_id = %session_info.id.0,
            error = %error,
            "failed to persist session model snapshot; using in-memory frozen values"
        );
    }
    SnapshotResolution {
        snapshot,
        kind: SnapshotResolutionKind::Created,
    }
}

/// Persist a new snapshot generation after an explicit setModel action.
pub fn persist_explicit_model_switch(
    session_info: &SessionInfo,
    config: &SamplerConfig,
    effective_context_window: u64,
) -> ResolvedSessionModelSnapshot {
    let snapshot = build_snapshot(config, effective_context_window);
    if let Err(error) = session_store(session_info).store(&snapshot) {
        tracing::warn!(
            session_id = %session_info.id.0,
            model = %config.model,
            error = %error,
            "failed to persist explicit model-switch snapshot"
        );
    }
    snapshot
}

/// Apply only the non-secret runtime contract. Endpoints, credentials,
/// request headers, retry policy and client identity remain freshly resolved.
pub fn apply_snapshot_to_sampler_config(
    config: &mut SamplerConfig,
    snapshot: &ResolvedSessionModelSnapshot,
) -> bool {
    if config.model != snapshot.model_id {
        tracing::warn!(
            active_model = %config.model,
            snapshot_model = %snapshot.model_id,
            "refusing to apply a session model snapshot for a different model"
        );
        return false;
    }
    config.api_backend = snapshot.api_backend.clone();
    config.provider_extensions = snapshot.provider_extensions.clone();
    config.context_window = snapshot.context_window;
    config.max_completion_tokens = snapshot.max_completion_tokens;
    true
}

fn session_store(session_info: &SessionInfo) -> SessionModelSnapshotStore {
    SessionModelSnapshotStore::new(
        crate::session::persistence::session_dir(session_info).join(SNAPSHOT_DIRECTORY),
    )
}

fn build_snapshot(
    config: &SamplerConfig,
    effective_context_window: u64,
) -> ResolvedSessionModelSnapshot {
    let resolved_at_unix_ms = unix_time_ms();
    let catalog = load_catalog_evidence(&config.model, resolved_at_unix_ms);
    ResolvedSessionModelSnapshot {
        schema_version: SESSION_MODEL_SNAPSHOT_SCHEMA_VERSION,
        model_id: config.model.clone(),
        api_backend: config.api_backend.clone(),
        provider_extensions: config.provider_extensions.clone(),
        context_window: effective_context_window.max(1),
        max_completion_tokens: config.max_completion_tokens,
        catalog_metadata: catalog.metadata,
        catalog_origin: catalog.origin,
        catalog_revision: catalog.revision,
        catalog_fetched_at_unix_ms: catalog.fetched_at_unix_ms,
        catalog_expires_at_unix_ms: catalog.expires_at_unix_ms,
        catalog_stale: catalog.stale,
        resolved_at_unix_ms,
    }
}

#[derive(Default)]
struct CatalogEvidence {
    metadata: Option<ModelMetadata>,
    origin: Option<String>,
    revision: Option<String>,
    fetched_at_unix_ms: Option<u64>,
    expires_at_unix_ms: Option<u64>,
    stale: bool,
}

fn load_catalog_evidence(model_id: &str, now_unix_ms: u64) -> CatalogEvidence {
    let cached = match default_model_catalog_cache().load_latest(None, now_unix_ms) {
        Ok(Some(cached)) => cached,
        Ok(None) => return CatalogEvidence::default(),
        Err(error) => {
            tracing::warn!(
                model = model_id,
                error = %error,
                "failed to load model catalog while freezing session metadata"
            );
            return CatalogEvidence::default();
        }
    };
    CatalogEvidence {
        metadata: cached
            .document
            .models
            .iter()
            .find(|metadata| metadata.model_id == model_id)
            .cloned(),
        origin: Some(cached.document.origin.clone()),
        revision: cached.document.revision.clone(),
        fetched_at_unix_ms: Some(cached.document.fetched_at_unix_ms),
        expires_at_unix_ms: Some(cached.document.expires_at_unix_ms),
        stale: matches!(cached.freshness, CatalogFreshness::Stale),
    }
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn is_snapshot_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("snapshot-") && name.ends_with(".json"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn config(model: &str, context_window: u64, max_output: Option<u32>) -> SamplerConfig {
        SamplerConfig {
            model: model.to_string(),
            context_window,
            max_completion_tokens: max_output,
            ..Default::default()
        }
    }

    fn test_snapshot(
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
        let snapshot = test_snapshot("company/model", 262_144, Some(65_536), 100);
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
            .store(&test_snapshot("model-a", 128_000, Some(8_000), 100))
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
    fn newest_model_switch_generation_wins() {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionModelSnapshotStore::new(temp.path()).with_max_entries(4);
        store
            .store(&test_snapshot("model-a", 128_000, Some(8_000), 100))
            .unwrap();
        store
            .store(&test_snapshot("model-b", 256_000, Some(16_000), 200))
            .unwrap();

        let restored = store.load_latest().unwrap().unwrap();
        assert_eq!(restored.model_id, "model-b");
        assert_eq!(restored.context_window, 256_000);
    }

    #[test]
    fn restored_snapshot_overrides_refreshed_runtime_limits() {
        let snapshot = test_snapshot("model-a", 128_000, Some(8_000), 100);
        let mut refreshed = config("model-a", 512_000, Some(64_000));
        assert!(apply_snapshot_to_sampler_config(&mut refreshed, &snapshot));
        assert_eq!(refreshed.context_window, 128_000);
        assert_eq!(refreshed.max_completion_tokens, Some(8_000));
    }

    #[test]
    fn snapshot_for_another_model_is_rejected() {
        let snapshot = test_snapshot("model-a", 128_000, Some(8_000), 100);
        let mut config = config("model-b", 256_000, Some(16_000));
        assert!(!apply_snapshot_to_sampler_config(&mut config, &snapshot));
        assert_eq!(config.context_window, 256_000);
    }
}
