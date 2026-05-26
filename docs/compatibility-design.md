# Compatibility Design

This document records the initial engineering design for a Rust service that
accepts Anthropic Messages API requests from Claude Code and forwards them to
DeepSeek's Anthropic-compatible endpoint. The mapping is intentionally one-way:
Anthropic Messages API in, DeepSeek API out.

DeepSeek already provides `https://api.deepseek.com/anthropic`, so the first
implementation should be a compatibility shim, not a full Anthropic-to-OpenAI
protocol reimplementation.

## Target Scope

Implement:

- `POST /v1/messages`
- `POST /v1/messages/count_tokens`
- optional `GET /v1/models` for Claude Code gateway model discovery
- non-stream responses with exact Anthropic Messages response shape
- streamed responses with exact Anthropic SSE event semantics
- Claude model name mapping to DeepSeek V4 models
- text messages
- tool definitions, tool choice, tool use, and tool results via pass-through
- basic usage and error normalization

Defer or explicitly reject:

- image input
- document input
- MCP content blocks
- code execution blocks
- file API integration
- full prompt caching semantics
- reverse DeepSeek-to-Anthropic gateway behavior

## Configuration

Read from `.env`:

```text
DEEPSEEK_API_KEY=
DEEPSEEK_BASE_URL=https://api.deepseek.com/anthropic
DEEPSEEK_UPSTREAM_PROTOCOL=anthropic
SERVER_HOST=127.0.0.1
SERVER_PORT=3000
DEFAULT_DEEPSEEK_MODEL=deepseek-v4-flash
CLAUDE_OPUS_MODEL=deepseek-v4-pro
CLAUDE_SONNET_MODEL=deepseek-v4-flash
CLAUDE_HAIKU_MODEL=deepseek-v4-flash
DEEPSEEK_THINKING=disabled
DEEPSEEK_REASONING_EFFORT=high
```

The client-facing `x-api-key` can be treated in one of two ways:

- simple local mode: accept any non-empty key and always use `DEEPSEEK_API_KEY`
- gateway mode: require a configured proxy key and reject mismatches

The initial implementation should use simple local mode unless multi-user access
is added.

## Model Mapping

Default mapping:

| Incoming model prefix | DeepSeek model |
| --- | --- |
| `claude-opus` | `deepseek-v4-pro` |
| `claude-sonnet` | `deepseek-v4-flash` |
| `claude-haiku` | `deepseek-v4-flash` |
| `deepseek-v4-pro` | `deepseek-v4-pro` |
| `deepseek-v4-flash` | `deepseek-v4-flash` |
| anything else | `DEFAULT_DEEPSEEK_MODEL` |

Response policy:

- `model` in the Anthropic response should echo the incoming model name by
  default, because many Anthropic clients use it as a client-side capability
  marker.
- Log or expose the backend model separately only in diagnostics.

## Request Forwarding And Patching

Anthropic request:

```json
{
  "model": "claude-sonnet-4-5",
  "max_tokens": 1024,
  "system": "You are concise.",
  "messages": [
    { "role": "user", "content": "Hello" }
  ]
}
```

Primary DeepSeek request:

```json
{
  "model": "claude-sonnet-4-5",
  "max_tokens": 1024,
  "thinking": { "type": "enabled" },
  "output_config": { "effort": "high" },
  "system": "You are concise.",
  "messages": [
    { "role": "user", "content": "Hello" }
  ]
}
```

The request remains Anthropic-shaped because the upstream endpoint is
Anthropic-compatible. The gateway should only patch fields when needed.

Field policy:

| Anthropic field | DeepSeek Anthropic field | Notes |
| --- | --- | --- |
| `model` | `model` | pass Claude names or force mapped DeepSeek name by config |
| `max_tokens` | `max_tokens` | pass through |
| `system` | `system` | pass through |
| `messages` | `messages` | pass through, reject known unsupported blocks |
| `stop_sequences` | `stop_sequences` | pass through |
| `stream` | `stream` | pass through |
| `temperature` | `temperature` | pass through |
| `top_p` | `top_p` | pass through |
| `metadata.user_id` | `metadata.user_id` | pass through |
| `tools` | `tools` | pass through |
| `tool_choice` | `tool_choice` | pass through |
| `thinking.type` | `thinking.type` | optional pass through |
| `output_config.effort` | `output_config.effort` | map configured aliases if needed |

Unsupported Anthropic fields should be ignored only when the official DeepSeek
Anthropic endpoint ignores them. For unsupported content types, prefer a clear
`400` error over silently dropping user input.

## Content Block Conversion

Input content policy:

- string content: pass through
- `text` blocks: pass through
- `thinking` blocks: pass through only after live Claude Code testing
- `tool_use` blocks: pass through
- `tool_result` blocks: pass through
- `image`, `document`, `search_result`, `mcp_tool_use`, `code_execution*`:
  return `400 unsupported_content_block`

Assistant history:

- text blocks remain text blocks
- `tool_use` blocks remain tool use blocks
- reasoning or thinking blocks should be preserved only if Claude Code and
  DeepSeek both handle them correctly in live tests

## Tool Pass-Through

DeepSeek's Anthropic endpoint supports Anthropic-format tools directly:

```json
{
  "name": "get_weather",
  "description": "Get weather",
  "input_schema": {
    "type": "object",
    "properties": {
      "city": { "type": "string" }
    },
    "required": ["city"]
  }
}
```

Tool choice policy:

| Anthropic | DeepSeek |
| --- | --- |
| `{ "type": "none" }` | pass through |
| `{ "type": "auto" }` | pass through |
| `{ "type": "any" }` | pass through |
| `{ "type": "tool", "name": "x" }` | pass through |

DeepSeek documents `disable_parallel_tool_use` as ignored. If Claude Code relies
on disabled parallel tool use, the gateway may need to enforce it locally by
patching or rejecting multi-tool responses.

## Response Conversion

DeepSeek Anthropic-compatible non-stream response should already be
Anthropic-shaped:

```json
{
  "id": "msg_...",
  "type": "message",
  "role": "assistant",
  "content": [{ "type": "text", "text": "Hello" }],
  "model": "deepseek-v4-flash",
  "stop_reason": "end_turn",
  "stop_sequence": null,
  "usage": {
    "input_tokens": 16,
    "output_tokens": 10
  }
}
```

Gateway response:

```json
{
  "id": "msg_...",
  "type": "message",
  "role": "assistant",
  "content": [{ "type": "text", "text": "Hello" }],
  "model": "claude-sonnet-4-5",
  "stop_reason": "end_turn",
  "stop_sequence": null,
  "usage": {
    "input_tokens": 16,
    "output_tokens": 10
  }
}
```

Patch policy:

- echo the incoming Claude model name in `model` if Claude Code requires it
- preserve Anthropic content block shapes
- preserve usage when already Anthropic-shaped
- normalize only malformed or Claude Code-incompatible errors

Non-streaming is a compatibility target, not a fallback. Validate:

- top-level `type = "message"`
- `role = "assistant"`
- `content[]` contains only Claude Code-supported blocks or explicit errors
- `stop_reason` is one of the Anthropic values Claude Code expects
- `usage.input_tokens` and `usage.output_tokens` exist when upstream provides
  usage

## Streaming Conversion

DeepSeek's Anthropic endpoint should already emit named Anthropic SSE events:

```text
event: message_start
data: {"type":"message_start","message":{...}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":10}}

event: message_stop
data: {"type":"message_stop"}
```

Implementation notes:

- pass Anthropic SSE events through by default
- validate event names and JSON shape during development tests
- patch only if Claude Code rejects a DeepSeek event shape
- if a future provider uses OpenAI chunks, use a separate provider adapter that
  builds these Anthropic events

Streaming validation must cover full event order:

1. `message_start`
2. `content_block_start`
3. one or more `content_block_delta`
4. `content_block_stop`
5. `message_delta`
6. `message_stop`

Tool streams must preserve `tool_use` block IDs and emit valid
`input_json_delta` fragments. Error streams must use Anthropic `error` events
rather than leaking upstream transport details.

## Error Normalization

Return Anthropic-like JSON errors:

```json
{
  "type": "error",
  "error": {
    "type": "invalid_request_error",
    "message": "unsupported content block: image"
  }
}
```

Suggested status mapping:

| Condition | HTTP | Anthropic error type |
| --- | --- | --- |
| invalid JSON/body | 400 | `invalid_request_error` |
| unsupported content block | 400 | `invalid_request_error` |
| missing/invalid client API key | 401 | `authentication_error` |
| DeepSeek auth failure | 502 | `api_error` |
| DeepSeek rate limit | 429 | `rate_limit_error` |
| DeepSeek timeout/network | 529 or 502 | `overloaded_error` or `api_error` |

## Rust Implementation Shape

Recommended crates:

- `axum` for HTTP routing and SSE responses
- `tokio` for async runtime
- `reqwest` for DeepSeek upstream requests
- `serde` and `serde_json` for request/response models
- `dotenvy` for `.env`
- `tracing` and `tracing-subscriber` for logs
- `thiserror` for typed conversion errors

Suggested modules:

```text
src/
  main.rs
  config.rs
  anthropic.rs
  deepseek.rs
  convert/
    mod.rs
    request.rs
    response.rs
    stream.rs
  error.rs
```

The conversion layer should be pure and unit-tested. HTTP handlers should mostly
parse, call conversion, forward upstream, and convert the result back.

## Initial Test Matrix

Unit tests:

- Claude model prefix mapping
- top-level `system` to DeepSeek system message
- text block flattening
- unsupported content block rejection
- tool definition conversion
- tool choice conversion
- DeepSeek finish reason mapping
- DeepSeek usage mapping

Integration-style tests with mocked upstream:

- non-stream text response
- non-stream tool_use response
- stream text response with proper Anthropic event order
- upstream error normalization
