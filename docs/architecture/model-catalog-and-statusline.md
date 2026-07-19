# DTTN Dynamic Model Catalog, Pricing, Account Telemetry and Status Line

## Goals

1. Resolve model context limits, output limits, protocol and capabilities from the configured provider whenever the provider exposes authoritative metadata.
2. Resolve model pricing from an authoritative provider source or an auditable registry without hard-coding every model in the DTTN binary.
3. Show provider balance, quota and rate-limit information when a supported account endpoint exists.
4. Preserve deterministic, offline-safe startup when metadata, pricing or account telemetry is unavailable.
5. Display operationally useful session state without confusing current-context usage, cumulative session usage, estimated cost and provider-reported balance.

## Non-goals

- Scraping arbitrary provider documentation pages at runtime.
- Trusting model-name heuristics as authoritative metadata.
- Blocking Agent startup on a metadata or billing refresh.
- Sending API credentials to a third-party model registry.
- Treating an unsupported balance endpoint as a zero balance.
- Claiming a locally estimated token cost is identical to the provider invoice.

## Source precedence

DTTN resolves every model field independently. A provider may know the context window but not strict JSON Schema support, while a company registry may know both. Pricing can also have a different source from capabilities.

Precedence, highest first:

1. User or administrator override.
2. Authenticated enterprise registry.
3. Configured provider API.
4. Audited public registry with recorded origin and revision.
5. Verified local cache.
6. Built-in distribution metadata.
7. Unknown.

A lower-priority source never overwrites a stronger value. Equal-priority conflicts keep the first value and emit a structured warning. Every externally sourced field should retain a redacted origin and revision or ETag so `doctor model` can explain where it came from.

## Provider discovery adapters

A single generic `GET /v1/models` implementation is insufficient. Each provider adapter returns a partial normalized model record and declares which fields are authoritative.

### OpenAI-compatible

`GET /v1/models` is used for availability and ownership only unless the configured provider returns documented extension fields. The standard model object does not guarantee context, output limits, capabilities or pricing.

For OpenAI-compatible services without richer metadata, DTTN resolves missing fields from the enterprise registry, audited public registry, cache or explicit override. It does not guess limits from the model name.

### Gemini

`models.get/list` can expose input and output token limits, supported generation methods and sampling defaults. Those values can be mapped directly when returned by the configured provider.

### Mistral

`GET /v1/models` can expose `max_context_length` and capability flags such as function calling and vision. Missing pricing still requires a pricing source.

### Anthropic

The models endpoint is used for identity and availability. Context, capabilities and pricing are enriched only from documented fields, an enterprise registry or an audited registry.

### DTTN enterprise registry

A company-managed registry is the recommended normalization point for production deployments. It can proxy official discovery APIs and publish one stable schema:

```json
{
  "schema_version": 1,
  "generated_at": "2026-07-19T00:00:00Z",
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
      "pricing": {
        "currency": "USD",
        "input_per_million_microunits": 2000000,
        "cached_input_per_million_microunits": 500000,
        "output_per_million_microunits": 8000000
      },
      "origin": "provider-official-api",
      "source_revision": "etag-or-release-id"
    }
  ]
}
```

Prices use micro-units of the currency per one million tokens. For example, USD 2.00 per one million tokens is `2_000_000`. Integer storage prevents floating-point drift.

The registry response should be signed or delivered through an authenticated company endpoint. DTTN must never download executable code from it.

### Audited public registry

An open-source registry such as a reviewed models catalog can be used only as a fallback when the provider does not expose the field. Requirements:

- Pin a schema version and revision or commit.
- Record the original model ID and provider.
- Validate numeric ranges and reject impossible values.
- Never send provider credentials to the public registry.
- Rank it below the provider API and enterprise registry.
- Mark status-line cost as locally estimated.

## Refresh and cache policy

- Refresh asynchronously after configuration and credentials are loaded.
- Default model metadata refresh timeout: 5 seconds.
- Default account telemetry timeout: 3 seconds.
- Default model cache TTL: 24 hours.
- Default balance/quota TTL: 60 seconds unless the provider specifies otherwise.
- Cache verified model data under `DTTN_HOME/cache/model-catalog-v1.json`.
- Do not persist provider balances by default; keep them in memory unless an administrator explicitly enables encrypted persistence.
- Use atomic write-and-rename for model cache updates.
- Keep the previous cache when refresh fails validation.
- Record source, fetch time, schema version, endpoint origin and revision.
- Never cache API keys, Authorization headers, cookies or full error bodies.
- A stale model cache may be used with a visible warning.
- An unknown context window remains unknown rather than silently becoming a guessed default.

## Session-frozen model snapshot

The model selected for a session is frozen to a resolved metadata snapshot containing:

- routing model ID and provider ID;
- protocol and provider extensions;
- context and output limits;
- capability flags;
- pricing revision and source;
- metadata fetch time and stale state.

A background refresh affects only new sessions unless an explicit model reload is requested. This prevents a provider change from altering compaction thresholds or cost calculations midway through a task.

Compaction should use:

1. resolved total context window;
2. reserved maximum output;
3. system and tool-schema overhead;
4. configurable safety margin.

It must not use cumulative session token totals as current context usage.

## Cost calculation

DTTN normalizes billable usage into separate buckets:

- uncached input tokens;
- cached input tokens;
- output tokens;
- separately billable reasoning tokens.

Reasoning tokens must not be placed in a separate bucket when the provider already includes them in output tokens. The estimator returns a value only when every non-zero bucket has a known price. Missing prices hide the cost segment rather than showing a partial total.

The status line should distinguish:

- `est $0.84`: locally calculated from token usage and a known price table;
- `billed $0.84`: provider-reported usage charge, when an API explicitly provides it;
- `balance $12.30`: provider account balance, not session cost.

## Provider account telemetry

Balance support is provider-specific and must use a dedicated adapter rather than model-name matching.

The normalized state is one of:

- `available`;
- `unsupported`;
- `auth_required`;
- `permission_denied`;
- `temporarily_unavailable`;
- `disabled_by_policy`.

A successful snapshot may include:

- monetary balance;
- remaining quota and limit;
- reset time;
- request and token rate limits;
- redacted account label;
- fetch and expiry timestamps.

Rendering rules:

- Only `available` and fresh values may be displayed.
- `unsupported` is hidden by default and shown as `balance n/a` only in expanded diagnostics.
- Authentication and permission failures must not repeatedly retry in the render loop.
- Billing requests run outside the UI render path with strict timeout, cancellation and backoff.
- A provider adapter must declare the endpoint, auth scope and whether the value is prepaid balance, monthly budget, quota or rate limit.

## Status line

The existing composable status bar should remain the renderer. New data arrives through a redacted runtime snapshot; the render path performs no network, filesystem or Git subprocess work.

### Default compact view

```text
agnes-2.0-flash · ctx 61% left · in 42.1K/out 6.3K · est $0.08 · 820ms · main*
```

When a supported provider exposes balance:

```text
deepseek-chat · ctx 72% left · est ¥0.03 · balance ¥18.42 · 640ms · main*
```

### Optional expanded view

```text
agnes-2.0-flash [standard/chat] · ctx 102K/256K, 61% left · session 1.4M tokens
est $0.84 [provider_api] · balance n/a [unsupported] · main* · workspace-write
2 tools running · compacted 1x · last 820ms / TTFT 210ms · RPM 47 reset 18s
```

### Built-in segments

High priority:

- `model`: routing model and optional reasoning effort.
- `context`: current context used, total and exact percentage remaining.
- `run-state`: idle, sampling, tool, waiting approval, retrying or compacting.
- `git`: branch and dirty state.

Medium priority:

- `turn-tokens`: input, cached input, output and reasoning tokens from the latest call.
- `cost`: known or estimated session cost with source marker.
- `balance`: provider balance or quota when supported and fresh.
- `latency`: latest total latency and time to first token.
- `tools`: active and queued tool count.
- `limits`: provider rate-limit remaining and reset time.

Lower priority or expanded-only:

- `provider`: provider identity, protocol and metadata source.
- `session-tokens`: cumulative session totals, clearly labelled cumulative.
- `cwd`: project-relative directory.
- `permission`: approval and sandbox mode.
- `compaction`: count and age of latest compaction.
- `metadata-age`: source revision and stale state.

### Width degradation

1. Hide expanded-only segments.
2. Collapse token detail to total input/output.
3. Hide balance before context, model, run state or Git branch.
4. Collapse latency from total plus TTFT to total only.
5. Shorten model aliases only when the alias mapping is explicit.
6. Never truncate monetary numbers in a way that changes their value.

### Rendering and security rules

- Never show both context used and context remaining by default.
- Always label cumulative token totals separately from current context usage.
- Hide unavailable fields rather than displaying fabricated zeroes.
- Git inspection must be cached and run outside the render path.
- Billing and metadata adapters must redact account IDs and endpoint query parameters from logs.
- External command-backed status lines, if added later, receive redacted structured JSON, have a strict timeout and are disabled in untrusted workspaces by default.

## Delivery phases

1. Pure metadata, pricing, account telemetry and merge contracts.
2. Provider discovery adapters and validated model cache.
3. Pricing registry adapter and provenance reporting.
4. Provider account adapters for explicitly supported providers.
5. Session-frozen resolved metadata and Doctor reporting.
6. Built-in configurable status-line segments.
7. Optional command-backed custom renderer.
