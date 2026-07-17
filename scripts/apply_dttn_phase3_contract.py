#!/usr/bin/env python3
"""Apply Phase 3 and preserve xAI-only doom-loop contract tests explicitly."""

import apply_dttn_phase3_verified  # noqa: F401 - executes deterministic migration
import apply_dttn_phase3 as migration

migration.replace_exact(
    "crates/codegen/xai-grok-sampler/tests/test_actor.rs",
    "    ConversationItem, ConversationRequest, DoomLoopRecoveryPolicy, UserItem,\n",
    "    ConversationItem, ConversationRequest, DoomLoopRecoveryPolicy, ProviderExtensions, UserItem,\n",
)

migration.replace_exact(
    "crates/codegen/xai-grok-sampler/tests/test_actor.rs",
    "    cfg.api_backend = ApiBackend::Responses;\n    cfg.doom_loop_recovery = doom_loop;",
    "    cfg.api_backend = ApiBackend::Responses;\n    cfg.provider_extensions = ProviderExtensions::Xai;\n    cfg.doom_loop_recovery = doom_loop;",
)

print("DTTN Phase 3 xAI-only test contract updated")
