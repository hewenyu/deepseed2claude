# Project Spec: Claude Code to DeepSeek V4 Compatibility Gateway

Date: 2026-05-26

## Goal

Build a Rust gateway that lets Claude Code use the DeepSeek API as if it were
talking to Anthropic's Messages API.

The product goal is not a general bidirectional protocol bridge. The only
required direction is:

```text
Claude Code / Anthropic Messages API client
  -> local Rust gateway
  -> DeepSeek Anthropic-compatible API
  -> DeepSeek V4 model
```

DeepSeek already provides an Anthropic-compatible endpoint at
`https://api.deepseek.com/anthropic`. The gateway should use that endpoint as the
primary upstream path, then patch only the parts needed for Claude Code
compatibility, local testing, observability, model routing, and future provider
adapters.

## Non-Goals

- Do not implement DeepSeek-to-Claude reverse mapping.
- Do not expose an OpenAI-compatible public API as the primary contract.
- Do not require Claude Code patches.
- Do not reimplement the whole Anthropic-to-OpenAI conversion while DeepSeek's
  hosted Anthropic endpoint is sufficient.
- Do not silently drop user-visible content that Claude Code expects to matter.
- Do not optimize for generic chat UI compatibility before Claude Code works.

## Primary Client

Claude Code is the compatibility target.

Claude Code officially supports routing requests through a proxy or gateway with
`ANTHROPIC_BASE_URL`. It sends API keys through `ANTHROPIC_API_KEY` as the
`X-Api-Key` header, and can send `Authorization: Bearer ...` with
`ANTHROPIC_AUTH_TOKEN`.

The gateway must therefore accept:

- `POST /v1/messages`
- `POST /v1/messages/count_tokens` if Claude Code requests token counting
- `GET /v1/models` if needed for model discovery
- `x-api-key`
- `authorization: Bearer ...`
- `anthropic-version`
- `anthropic-beta`
- Anthropic request/response JSON bodies
- Anthropic SSE event streams

Claude Code can disable MCP tool search by default when `ANTHROPIC_BASE_URL`
points to a non-first-party host. That means first-class local tool use through
Claude Code's normal tool schema is more important than relying on hosted
Anthropic-only extensions.

## Upstream Provider

Initial upstream provider:

- DeepSeek V4
- Anthropic-compatible endpoint: `https://api.deepseek.com/anthropic/v1/messages`
- models: `deepseek-v4-flash`, `deepseek-v4-pro`

DeepSeek's OpenAI-compatible endpoint,
`https://api.deepseek.com/chat/completions`, is a fallback implementation path
only if the Anthropic-compatible endpoint fails a Claude Code requirement that
cannot be fixed by a thin local shim.

## Future Provider Direction

The later architecture should support more upstream models, but each provider is
still a one-way adapter from Anthropic Messages into that provider's native API.

Provider adapters should share:

- Anthropic request models
- Anthropic response models
- Claude Code compatibility tests
- normalized error types
- normalized stream event builder

Provider adapters should own:

- model mapping
- upstream auth
- request patching or conversion
- response patching or conversion
- stream event patching or chunk parsing
- provider-specific unsupported-feature decisions

## Configuration Contract

The local `.env` file is the gateway configuration source during development:

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

Expected Claude Code development setup:

```sh
export ANTHROPIC_BASE_URL=http://127.0.0.1:3000
export ANTHROPIC_API_KEY=local-dev-key
claude
```

`ANTHROPIC_API_KEY` is a client-to-gateway key. It is not the DeepSeek key. The
gateway reads `DEEPSEEK_API_KEY` from `.env` and sends it upstream as
`x-api-key: <key>` to the DeepSeek Anthropic-compatible endpoint.

## Compatibility Requirements

The gateway is compatible when Claude Code can complete normal coding sessions
against DeepSeek V4 without client-side protocol errors.

Minimum required behavior:

- Claude Code can start with `ANTHROPIC_BASE_URL` pointing to the local gateway.
- Claude Code can send non-stream and stream `POST /v1/messages` requests, and
  both modes are first-class compatibility targets.
- Non-stream responses exactly preserve Anthropic Messages response shape:
  message envelope, content blocks, stop reason, stop sequence, model, and usage.
- Assistant text streams render incrementally in Claude Code.
- Stream responses exactly preserve Anthropic SSE semantics: event names, event
  ordering, content block lifecycle, text deltas, tool input deltas, final
  message delta, usage, and `message_stop`.
- Claude Code tool definitions pass through to DeepSeek Anthropic-compatible
  tools.
- DeepSeek tool calls are returned to Claude Code as Anthropic `tool_use`
  blocks.
- Claude Code `tool_result` blocks pass through or are minimally normalized.
- Long coding turns do not leak DeepSeek `reasoning_content` into normal text.
- DeepSeek stop reasons are normalized to Anthropic stop reasons.
- DeepSeek usage fields are normalized to Anthropic usage fields.
- Unsupported Anthropic content blocks fail with clear Anthropic-shaped errors.
- Upstream DeepSeek errors are normalized so Claude Code receives usable error
  messages instead of OpenAI-shaped implementation details.

## Claude Code Tool Use Requirements

Tool use is a hard requirement, not an optional later feature. Claude Code's core
coding workflow depends on tool calls.

The gateway must support this loop:

1. Claude Code sends `tools[]` in Anthropic format.
2. Gateway forwards them to DeepSeek's Anthropic-compatible endpoint.
3. DeepSeek returns Anthropic-format tool calls.
4. Gateway validates or patches Anthropic `tool_use` content blocks.
5. Claude Code executes local tools.
6. Claude Code sends `tool_result` content blocks.
7. Gateway forwards or minimally patches them for DeepSeek.
8. DeepSeek continues the assistant turn.

Tool-call IDs must remain stable across the round trip. Tool input JSON must
stay JSON and must not be stringified twice. If DeepSeek's streaming tool events
ever differ from Claude Code's expectations, the gateway patches the stream into
valid Anthropic `input_json_delta` events.

## Streaming Requirements

Claude Code expects Anthropic SSE events. DeepSeek's Anthropic-compatible
endpoint should already return Anthropic SSE events. The gateway must pass these
through by default and patch only if Claude Code exposes incompatibilities.

The gateway must preserve or produce:

- `message_start`
- `content_block_start`
- `content_block_delta`
- `content_block_stop`
- `message_delta`
- `message_stop`
- `error` when needed

Text deltas should remain `text_delta`. Tool input deltas should remain
`input_json_delta`. Final usage should be preserved when DeepSeek provides it.

Streaming compatibility is not allowed to be best-effort. The gateway must keep
Claude Code's parser happy for text-only turns, tool-call turns, tool-result
continuations, upstream errors, and early disconnects.

## Non-Streaming Requirements

Non-streaming compatibility is equally strict. The gateway must preserve or
patch DeepSeek's Anthropic-compatible JSON response into a valid Anthropic
Messages response for every supported content shape:

- text-only assistant messages
- assistant messages ending in `tool_use`
- assistant messages after `tool_result` continuation
- `end_turn`, `max_tokens`, `stop_sequence`, `tool_use`, and refusal-like stops
- usage accounting with `input_tokens` and `output_tokens`

The gateway should not treat non-streaming as a debug path. Claude Code and other
Anthropic clients may use either mode depending on command, model, or retry
behavior.

## Reasoning Policy

DeepSeek V4 thinking mode is useful for quality, but raw `reasoning_content` is
not normal assistant text.

Default policy:

- do not enable DeepSeek thinking mode by default for Claude Code traffic
- send configured `reasoning_effort` only when thinking is enabled/requested
- rely on DeepSeek's Anthropic-compatible `thinking` block behavior first
- suppress or patch only if Claude Code displays incompatible reasoning content
- do not promise Anthropic thinking-block compatibility until tested with Claude
  Code

## Acceptance Tests

Local mocked-upstream tests:

- basic Claude Code-style text request
- non-stream text response with exact Anthropic response shape
- streamed text request with exact Anthropic event order
- tool definition pass-through
- non-stream tool call response
- streamed tool call pass-through or patching
- `tool_result` continuation
- model prefix mapping
- unsupported image/document block rejection
- upstream auth failure normalization
- DeepSeek rate limit normalization

Live DeepSeek tests after `DEEPSEEK_API_KEY` is filled:

- non-stream text completion
- stream text completion
- non-stream simple tool-call round trip
- streamed simple tool-call round trip
- multi-step coding-like prompt using Claude Code against the local gateway
- Claude Code `stream-json` output against the local gateway

The live tests are not a replacement for mocked protocol tests. Mocked tests are
the source of truth for exact Claude Code wire compatibility.

## References

- Claude Code environment variables:
  https://code.claude.com/docs/en/env-vars
- DeepSeek Anthropic API compatibility:
  https://api-docs.deepseek.com/guides/anthropic_api
- DeepSeek Chat Completion API:
  https://api-docs.deepseek.com/api/create-chat-completion
- DeepSeek V4 models and pricing:
  https://api-docs.deepseek.com/quick_start/pricing
- Anthropic Messages API:
  https://docs.anthropic.com/en/api/messages
- Anthropic streaming:
  https://docs.anthropic.com/en/docs/build-with-claude/streaming
