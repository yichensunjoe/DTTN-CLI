from __future__ import annotations

import sys
import textwrap
from pathlib import Path


def replace_exact(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: phase5_session_snapshot_postprocess.py <script>")

    path = Path(sys.argv[1])
    script = path.read_text()

    ambiguous_spawn = (
        "replace_once(\n"
        "    spawn_path,\n"
        '    "    sampling_config: SamplingConfig,\\n",\n'
        '    "    mut sampling_config: SamplingConfig,\\n",\n'
        ")\n"
    )
    targeted_spawn = (
        "replace_once(\n"
        "    spawn_path,\n"
        "    '''pub(crate) async fn spawn_session_actor(\n"
        "    session_info: SessionInfo,\n"
        "    gateway: GatewaySender,\n"
        "    sampling_config: SamplingConfig,\n"
        "''',\n"
        "    '''pub(crate) async fn spawn_session_actor(\n"
        "    session_info: SessionInfo,\n"
        "    gateway: GatewaySender,\n"
        "    mut sampling_config: SamplingConfig,\n"
        "''',\n"
        ")\n"
    )
    script = replace_exact(script, ambiguous_spawn, targeted_spawn, "spawn signature patch")

    script = script.replace(
        "cargo test -p xai-grok-shell session_model_snapshot --lib",
        "cargo test -p xai-grok-shell --test session_model_snapshot_contract",
    )

    git_add_marker = (
        "  docs/architecture/session-frozen-model-metadata.md \\\n"
        "  crates/codegen/xai-grok-shell/src/session/mod.rs \\\n"
    )
    git_add_replacement = (
        "  docs/architecture/session-frozen-model-metadata.md \\\n"
        "  crates/codegen/xai-grok-shell/tests/session_model_snapshot_contract.rs \\\n"
        "  crates/codegen/xai-grok-shell/src/session/mod.rs \\\n"
    )
    script = replace_exact(script, git_add_marker, git_add_replacement, "git add patch")

    injection = textwrap.dedent(
        r"""
        python3 - <<'PY'
        from pathlib import Path

        setup = Path("crates/codegen/xai-grok-shell/src/session/acp_session_impl/session_setup.rs")
        setup_text = setup.read_text()
        setup_marker = "    /// Record the current time as the last API request timestamp.\n"
        if setup_text.count(setup_marker) != 1:
            raise SystemExit("failed to locate session metadata timestamp marker")
        setup_text = setup_text.replace(
            setup_marker,
            "    pub(super) const IDLE_REFRESH_THRESHOLD_SECS: i64 = 600;\n" + setup_marker,
            1,
        )
        local_const = "        const IDLE_REFRESH_THRESHOLD_SECS: i64 = 600;\n"
        if setup_text.count(local_const) != 1:
            raise SystemExit("failed to locate local idle refresh constant")
        setup_text = setup_text.replace(local_const, "", 1)
        setup_text = setup_text.replace(
            "if idle_secs >= IDLE_REFRESH_THRESHOLD_SECS {",
            "if idle_secs >= Self::IDLE_REFRESH_THRESHOLD_SECS {",
            1,
        )
        setup.write_text(setup_text)

        module = Path("crates/codegen/xai-grok-shell/src/session/session_model_snapshot.rs")
        module_text = module.read_text()
        test_marker = "mod tests {\n    use super::*;\n"
        if module_text.count(test_marker) != 1:
            raise SystemExit("failed to locate session snapshot unit tests")
        module.write_text(
            module_text.replace(
                test_marker,
                "mod tests {\n    use std::fs;\n\n    use super::*;\n",
                1,
            )
        )
        PY

        cat > crates/codegen/xai-grok-shell/tests/session_model_snapshot_contract.rs <<'RS'
        use std::fs;

        use xai_grok_sampler::SamplerConfig;
        use xai_grok_shell::session::session_model_snapshot::{
            ResolvedSessionModelSnapshot, SESSION_MODEL_SNAPSHOT_SCHEMA_VERSION,
            SessionModelSnapshotStore, apply_snapshot_to_sampler_config,
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
                temp.path().join("snapshot-99999999999999999999-corrupt.json"),
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
        RS

        """
    ).lstrip()

    rustfmt_marker = "rustfmt --edition 2024 \\\n"
    script = replace_exact(
        script,
        rustfmt_marker,
        injection + rustfmt_marker,
        "rustfmt injection point",
    )
    path.write_text(script)


if __name__ == "__main__":
    main()
