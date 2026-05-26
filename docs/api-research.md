# DeepSeek V4 to Anthropic Messages API Research

Date: 2026-05-26

This repository is intended to expose an Anthropic-compatible `POST /v1/messages`
surface for Claude Code backed by DeepSeek V4. The primary upstream surface is
DeepSeek's Anthropic-compatible endpoint.

- Anthropic Messages API: `POST https://api.anthropic.com/v1/messages`
- DeepSeek Anthropic-compatible API: base URL `https://api.deepseek.com/anthropic`
- DeepSeek OpenAI-compatible API fallback: `POST https://api.deepseek.com/chat/completions`

## Sources Checked

- DeepSeek Anthropic API compatibility guide:
  https://api-docs.deepseek.com/guides/anthropic_api
- DeepSeek Chat Completion API reference:
  https://api-docs.deepseek.com/api/create-chat-completion
- DeepSeek Models & Pricing:
  https://api-docs.deepseek.com/quick_start/pricing
- DeepSeek Thinking Mode:
  https://api-docs.deepseek.com/guides/thinking_mode
- Anthropic Messages guide and examples:
  https://platform.claude.com/docs/en/build-with-claude/working-with-messages
- Anthropic API overview:
  https://docs.anthropic.com/en/api/overview
- Anthropic streaming guide:
  https://docs.anthropic.com/en/docs/build-with-claude/streaming
- Claude Code environment variables:
  https://code.claude.com/docs/en/env-vars

## Current DeepSeek V4 Model Surface

DeepSeek documents two V4 API model IDs:

- `deepseek-v4-flash`
- `deepseek-v4-pro`

Both are available through the OpenAI-format base URL `https://api.deepseek.com`
and the Anthropic-format base URL `https://api.deepseek.com/anthropic`.

DeepSeek states that the old model names `deepseek-chat` and
`deepseek-reasoner` will be deprecated. For compatibility, they currently map to
the non-thinking and thinking modes of `deepseek-v4-flash`.

As of the researched docs, the V4 models advertise:

- context length: `1M`
- max output: `384K`
- JSON output support
- tool call support
- chat prefix completion beta support
- FIM completion in non-thinking mode only

## DeepSeek Native Anthropic Endpoint

DeepSeek already exposes an Anthropic-format endpoint:

```text
ANTHROPIC_BASE_URL=https://api.deepseek.com/anthropic
ANTHROPIC_API_KEY=<deepseek-api-key>
```

The official mapping for Claude model names is prefix based:

- `claude-opus*` -> `deepseek-v4-pro`
- `claude-haiku*` -> `deepseek-v4-flash`
- `claude-sonnet*` -> `deepseek-v4-flash`
- unsupported names -> `deepseek-v4-flash`

This should be the primary upstream for the local Rust gateway. A local gateway
is still useful for stricter Claude Code compatibility, local testing,
observability, custom model mapping, future providers, and workarounds for any
Anthropic details not fully preserved by DeepSeek's hosted compatibility layer.

## Anthropic Messages API Shape

Request basics:

- authentication uses `x-api-key`
- `anthropic-version` is required by the Anthropic API
- request and response bodies are JSON
- endpoint is `POST /v1/messages`
- request contains `model`, `max_tokens`, and `messages`
- `system` is a top-level field, not a `messages[]` role
- `messages[]` roles are `user` and `assistant`
- content may be a string or an array of typed blocks
- the API is stateless; clients resend conversation history
- `stream: true` returns Anthropic SSE events, not OpenAI chunks

Non-stream response shape:

```json
{
  "id": "msg_...",
  "type": "message",
  "role": "assistant",
  "content": [{ "type": "text", "text": "..." }],
  "model": "claude-...",
  "stop_reason": "end_turn",
  "stop_sequence": null,
  "usage": {
    "input_tokens": 12,
    "output_tokens": 6
  }
}
```

Important response stop reasons include:

- `end_turn`
- `max_tokens`
- `stop_sequence`
- `tool_use`
- `refusal`

The streaming format is event-oriented. A normal stream contains:

1. `message_start`
2. for each content block: `content_block_start`, one or more
   `content_block_delta`, then `content_block_stop`
3. one or more `message_delta`
4. `message_stop`

Streams may also contain `ping` and `error` events. Unknown event types should be
handled gracefully.

## DeepSeek Chat Completion Shape

Request basics:

- authentication uses `Authorization: Bearer <DEEPSEEK_API_KEY>`
- endpoint is `POST /chat/completions`
- request contains `model` and `messages`
- supported model IDs are `deepseek-v4-flash` and `deepseek-v4-pro`
- `messages[]` roles include `system`, `user`, `assistant`, and `tool`
- `stream: true` returns OpenAI-style data-only SSE chunks and ends with
  `data: [DONE]`

Important request fields:

- `max_tokens`
- `response_format`
- `stop`
- `stream`
- `stream_options.include_usage`
- `temperature`
- `top_p`
- `tools`
- `tool_choice`
- `user_id`
- `thinking`
- `reasoning_effort`

DeepSeek finish reasons:

- `stop`
- `length`
- `content_filter`
- `tool_calls`
- `insufficient_system_resource`

Non-stream response shape:

```json
{
  "id": "...",
  "object": "chat.completion",
  "created": 1705651092,
  "model": "deepseek-v4-pro",
  "choices": [
    {
      "index": 0,
      "finish_reason": "stop",
      "message": {
        "role": "assistant",
        "content": "Hello",
        "reasoning_content": "...",
        "tool_calls": []
      }
    }
  ],
  "usage": {
    "prompt_tokens": 16,
    "completion_tokens": 10,
    "total_tokens": 26,
    "completion_tokens_details": {
      "reasoning_tokens": 0
    }
  }
}
```

## DeepSeek Thinking Mode

DeepSeek V4 thinking mode defaults to enabled. It can be controlled in the
OpenAI-compatible API with:

```json
{
  "thinking": { "type": "enabled" },
  "reasoning_effort": "high"
}
```

DeepSeek documents `high` and `max` effort. Compatibility mappings are:

- `low` and `medium` -> `high`
- `xhigh` -> `max`

In thinking mode, `temperature`, `top_p`, `presence_penalty`, and
`frequency_penalty` do not take effect, even if accepted for compatibility.

DeepSeek returns chain-of-thought-like content in `reasoning_content`. In normal
non-tool multi-turn conversations, prior `reasoning_content` can be omitted or is
ignored. If a thinking-mode assistant turn performs tool calls, DeepSeek requires
the associated `reasoning_content` to be passed back in subsequent requests;
otherwise the API can return `400`.

For a Claude-compatible proxy, the safest initial policy is:

- do not expose raw `reasoning_content` as user-visible `text`
- optionally map it to Anthropic `thinking` blocks only when the client requested
  thinking and the project explicitly supports that block type
- persist/pass through reasoning for tool-call continuation when needed

## DeepSeek Anthropic Compatibility Notes

DeepSeek's Anthropic-format docs state:

- `x-api-key` is fully supported
- `anthropic-version` is ignored
- `anthropic-beta` is ignored
- `max_tokens`, `stop_sequences`, `stream`, `system`, `temperature`, and `top_p`
  are fully supported
- `thinking` is supported but `budget_tokens` is ignored
- `output_config.effort` is supported
- `top_k`, `container`, `mcp_servers`, and `service_tier` are ignored
- `metadata.user_id` is supported; other metadata is ignored
- tools support `name`, `input_schema`, and `description`
- `tool_choice` supports `none`, `auto`, `any`, and named `tool`; the
  `disable_parallel_tool_use` flag is ignored
- text content blocks are supported
- image, document, search_result, redacted_thinking, code execution, and MCP
  content variants are not fully supported
- `tool_use` and `tool_result` blocks are supported

## Main Compatibility Gaps To Handle Locally

1. Model names:
   Anthropic clients send Claude model names. The proxy must map them to V4
   DeepSeek model IDs and decide whether the response `model` echoes the client
   model or reports the backend model.

2. Unsupported content:
   DeepSeek's Anthropic endpoint does not support every Anthropic content block
   variant. The gateway should reject unsupported user-visible blocks clearly
   instead of silently dropping them.

3. Claude Code behavior:
   Claude Code is stricter than a generic SDK client in some flows, especially
   streaming, non-stream response envelopes, and tool use. The gateway should
   patch only verified incompatibilities, but both stream and non-stream modes
   are required to work correctly.

4. Tools:
   DeepSeek supports Anthropic-format `tools`, `tool_choice`, `tool_use`, and
   `tool_result`, but `disable_parallel_tool_use` is ignored. If Claude Code
   depends on single-tool behavior, the gateway may need local enforcement.

5. Thinking:
   DeepSeek supports Anthropic `thinking`, but compatibility with Claude Code's
   display and continuation behavior needs live testing.

6. Error shape:
   DeepSeek errors should be normalized if Claude Code cannot handle the exact
   upstream body.

7. Future providers:
   Other providers may require full request, response, and streaming conversion.
   Keep that conversion isolated behind provider adapters instead of mixing it
   into the DeepSeek Anthropic fast path.

## Claude Code Routing Notes

Claude Code can route API requests through a proxy or gateway by setting
`ANTHROPIC_BASE_URL`. It sends `ANTHROPIC_API_KEY` as the `X-Api-Key` header and
supports `ANTHROPIC_AUTH_TOKEN` for a bearer `Authorization` header.

For this project, `ANTHROPIC_API_KEY` is only the client-to-local-gateway key.
The real DeepSeek credential stays in `.env` as `DEEPSEEK_API_KEY` and is sent
upstream by the gateway.

Claude Code may disable MCP tool search by default for non-first-party
`ANTHROPIC_BASE_URL` hosts. The gateway should therefore focus first on normal
Anthropic tool definitions, `tool_use`, and `tool_result` compatibility.
