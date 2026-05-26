# deepseed2claude

Claude Code -> local Rust gateway -> DeepSeek Anthropic-compatible API.

The gateway exposes Anthropic-compatible endpoints locally and forwards to
DeepSeek's `https://api.deepseek.com/anthropic` endpoint. It is intentionally
one-way: Claude/Anthropic Messages API in, DeepSeek API out.

## Run

Create `.env` from `.env.example` and fill `DEEPSEEK_API_KEY`.

```sh
cargo run --bin deepseed2claude
```

The default listener is:

```text
http://127.0.0.1:3000
```

Claude Code local configuration:

```sh
export ANTHROPIC_BASE_URL=http://127.0.0.1:3000
export ANTHROPIC_API_KEY=test
claude
```

## Supported Surface

- `POST /v1/messages`
- `POST /v1/messages/count_tokens`
- `GET /v1/models`
- non-stream Anthropic Messages responses
- stream Anthropic SSE responses
- text content
- Anthropic tool definitions, `tool_use`, and `tool_result`
- Claude model name mapping to DeepSeek V4 models
- upstream error normalization to Anthropic-shaped errors

DeepSeek thinking is disabled by default for Claude Code compatibility. Enable it
only after verifying the client can handle `thinking` blocks.

## Verification

```sh
cargo test
cargo clippy -- -D warnings
cargo run --bin live-check
```

`live-check` uses `.env` and validates direct DeepSeek non-stream and stream
responses. The gateway tests use a local mock upstream for exact protocol
behavior.

The implementation has also been verified with real Claude Code against the
local gateway:

```sh
ANTHROPIC_BASE_URL=http://127.0.0.1:3000 \
ANTHROPIC_API_KEY=test \
claude -p --bare --model claude-sonnet-4-5 \
  --permission-mode bypassPermissions \
  --allowedTools 'Read,Write,Bash' \
  'Read a file, write result.txt, run cat result.txt, and report the output.'
```

This covers Claude Code tool use through the gateway. Stream output was verified
with `--output-format stream-json --include-partial-messages`.
