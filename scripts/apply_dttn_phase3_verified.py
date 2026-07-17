#!/usr/bin/env python3
"""Apply the complete Phase 3 migration and update the DTTN User-Agent contract."""

import apply_dttn_phase3_complete  # noqa: F401 - executes deterministic migration
import apply_dttn_phase3 as migration

migration.replace_exact(
    "crates/codegen/xai-grok-sampler/src/client.rs",
    '        assert!(ua.starts_with("my-client grok-shell/"));',
    '        assert!(ua.starts_with("my-client dttn-cli/"));',
)

print("DTTN Phase 3 User-Agent contract updated")
