#!/usr/bin/env python3
"""Complete the Phase 3 migration with the remaining explicit test literal."""

import apply_dttn_phase3_clean  # noqa: F401 - executes deterministic migration
import apply_dttn_phase3 as migration

migration.replace_exact(
    "crates/codegen/xai-grok-sampler/tests/test_actor.rs",
    "        api_backend: ApiBackend::ChatCompletions,\n        auth_scheme: Default::default(),",
    "        api_backend: ApiBackend::ChatCompletions,\n        provider_extensions: Default::default(),\n        auth_scheme: Default::default(),",
)

print("DTTN Phase 3 sampler test migration completed")
