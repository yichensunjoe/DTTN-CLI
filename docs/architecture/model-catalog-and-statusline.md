# DTTN Dynamic Model Catalog and Status Line

## Goals

1. Resolve model context limits, output limits, protocol and capabilities from the configured provider whenever the provider exposes authoritative metadata.
2. Support models from multiple providers without hard-coding every model in the DTTN binary.
3. Preserve deterministic, offline-safe startup when a provider is unavailable.
4. Display operationally useful session state without confusing current-context usage with cumulative session usage.

## Non-goals

- Scraping arbitrary provider documentation pages at runtime.
- Trusting model-name heuristics as authoritative metadata.
- Blocking Agent startup on a metadata refresh.
- Sending API credentials to a third-party model registry.

## Metadata resolution

DTTN resolves each field independently. A provider may know the context window but not strict JSON Schema support, while the enterprise registry may know both.

Precedence, highest first:

1. User or administrator override.
2. Enterprise registry.
3. Configured provider API.
4. Verified local cache.
5. Built-in distribution metadata.
6. Unknown.

A lower-priority source never overwrites a stronger value. Equal-priority conflicts keep the first value and emit a structured warning.

## Provider adapters

### OpenAI-compatible

`GET /v1/models` is used for availability and ownership only. The standard OpenAI model object does not expose context or output limits, so these fields require an enterprise registry, provider-specific extension, cache or explicit override.

### Gemini

`models.get/list` exposes input and output token limits, supported generation methods, thinking support and sampling defaults. These values can be used directly as provider metadata.

### Mistral

`GET /v1/models` exposes `max_context_length` and capability flags including function calling and vision.

### Anthropic

The models endpoint is used for identity and availability. Context limits and capability policy are enriched from an enterprise registry or a provider-maintained adapter table unless the endpoint adds explicit fields.

### DTTN registry

A company-managed registry is the recommended normalization point for enterprise deployments. It may proxy provider discovery and return one stable schema:

```json
{
  "schema_version": 1,
  "models": [
    {
      "id": "provider/model",
      "protocol": "chat_completions",
      "context_window": 262144,
      "max_output_tokens": 65536,
      "capabilities": {
        "tool_calling": true,
        "parallel_tool_calls": false,
        "vision": true,
        "reasoning": true,
        "strict_json_schema": false,
        "streaming": true
      },
      "source_revision": "2026-07-18"
    }
  ]
}
```

The registry response should be signed or delivered through an authenticated company endpoint. DTTN must not download executable code from it.

## Refresh and cache policy

- Refresh asynchronously after configuration and credentials are loaded.
- Default refresh timeout: 5 seconds.
- Default cache TTL: 24 hours.
- Cache the last verified response under `DTTN_HOME/cache/model-catalog-v1.json`.
- Use atomic write-and-rename.
- Keep the previous cache when refresh fails validation.
- Record source, fetch time, schema version and endpoint origin.
- Never cache API keys, Authorization headers or full error bodies.
- A stale cache may be used with a visible warning; an unknown context window must remain unknown rather than silently becoming 200K.

## Runtime safety

The model selected for a session is frozen to a resolved metadata snapshot. A background refresh affects only new sessions unless an explicit model reload is requested. This prevents a provider metadata change from altering compaction thresholds midway through a task.

Compaction should use:

1. resolved total context window;
2. reserved maximum output;
3. system/tool-schema overhead;
4. configurable safety margin.

It must not use cumulative session token totals as current context usage.

## Status line

### Default compact view

```text
agnes-2.0-flash · ctx 61% left · in 42.1K/out 6.3K · 820ms · main*
```

### Optional expanded view

```text
agnes-2.0-flash [standard/chat] · ctx 102K/256K, 61% left · session 1.4M tokens · $0.84
main* · workspace-write · 2 tools running · compacted 1x · last 820ms / TTFT 210ms
```

### Built-in segments

- `model`: routing model and optional reasoning effort.
- `provider`: provider identity, protocol and metadata source.
- `context`: current context used, total and exact percentage remaining.
- `turn-tokens`: input, cached input, output and reasoning tokens from the latest model call.
- `session-tokens`: cumulative session totals, clearly labelled as cumulative.
- `cost`: known or estimated session cost with an `estimated` marker when prices are not provider-reported.
- `latency`: latest total latency and time to first token.
- `run-state`: idle, sampling, tool, waiting approval, retrying or compacting.
- `tools`: active and queued tool count.
- `git`: branch, dirty state, ahead and behind counts.
- `cwd`: project-relative directory.
- `permission`: approval and sandbox mode.
- `compaction`: count and age of the latest compaction.
- `limits`: provider rate-limit remaining and reset time when response headers expose them.

### Rendering rules

- Never show both `context-used` and `context-remaining` by default.
- Always label cumulative token totals separately from current context usage.
- Hide unavailable fields rather than displaying fabricated zeroes.
- Use width-aware priorities and truncate low-priority segments first.
- Git inspection must be cached and run outside the render path.
- External command-backed status lines, if added later, receive redacted structured JSON, have a strict timeout and are disabled in untrusted workspaces by default.

## Delivery phases

1. Pure metadata contract and merge tests.
2. Provider discovery adapters and cache.
3. Session-frozen resolved metadata and Doctor reporting.
4. Built-in status-line segments.
5. Optional command-backed custom renderer.
