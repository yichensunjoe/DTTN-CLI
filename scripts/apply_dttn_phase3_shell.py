#!/usr/bin/env python3
"""Apply Phase 3 and complete Shell model-catalog provider propagation."""

import apply_dttn_phase3_contract  # noqa: F401 - executes deterministic migration
import apply_dttn_phase3 as migration

migration.replace_exact(
    "crates/codegen/xai-grok-shell/src/agent/config.rs",
    "                api_backend: ApiBackend::Responses,\n                auth_scheme: Default::default(),",
    "                api_backend: ApiBackend::Responses,\n                provider_extensions: Default::default(),\n                auth_scheme: Default::default(),",
)

migration.replace_exact(
    "crates/codegen/xai-grok-shell/src/agent/config.rs",
    "            api_backend: ApiBackend::Responses,\n            auth_scheme: Default::default(),",
    "            api_backend: ApiBackend::Responses,\n            provider_extensions: Default::default(),\n            auth_scheme: Default::default(),",
)

migration.replace_exact(
    "crates/codegen/xai-grok-shell/src/remote/client.rs",
    '''        })
        .unwrap_or_default();
    Some(crate::agent::config::ModelEntryConfig {''',
    '''        })
        .unwrap_or_default();
    let provider_extensions = get_string(obj, "providerExtensions")
        .or_else(|| get_string(obj, "provider_extensions"))
        .and_then(|s| match s.as_str() {
            "auto" => Some(xai_grok_sampling_types::ProviderExtensions::Auto),
            "standard" => Some(xai_grok_sampling_types::ProviderExtensions::Standard),
            "xai" => Some(xai_grok_sampling_types::ProviderExtensions::Xai),
            _ => None,
        })
        .unwrap_or_default();
    Some(crate::agent::config::ModelEntryConfig {''',
)

migration.replace_exact(
    "crates/codegen/xai-grok-shell/src/remote/client.rs",
    "        api_backend,\n        context_window,",
    "        api_backend,\n        provider_extensions,\n        context_window,",
)

print("DTTN Phase 3 Shell provider propagation completed")
