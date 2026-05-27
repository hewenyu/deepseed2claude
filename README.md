# deepseed2claude

Claude Code -> local Rust gateway -> configured adapters.

The gateway exposes Anthropic-compatible endpoints locally, authenticates
Claude Code with administrator-managed client keys, and dispatches requests to
enabled adapters stored in SQLite.

## Run

Create `.env` from `.env.example` and set `ADMIN_USERNAME` /
`ADMIN_PASSWORD`. `DEEPSEEK_API_KEY` is optional for first boot; when present it
is seeded as the first DeepSeek adapter key.

```sh
npm --prefix admin-ui install
npm --prefix admin-ui run build
cargo run --bin deepseed2claude
```

The default listener is:

```text
http://127.0.0.1:3000
```

Open the admin UI:

```text
http://127.0.0.1:3000/admin
```

The admin UI manages:

- Adapters. Each adapter is one upstream key plus model mapping / thinking
  policy.
- Multiple Claude Code client keys. Use one of these values as
  `ANTHROPIC_API_KEY`.

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
- React admin UI embedded into the Rust binary at `/admin`
- SQLite-backed adapter and Claude Code client key management
- non-stream Anthropic Messages responses
- stream Anthropic SSE responses
- text content
- Anthropic tool definitions, `tool_use`, and `tool_result`
- Claude model name mapping to DeepSeek V4 models
- upstream error normalization to Anthropic-shaped errors

DeepSeek thinking defaults to `auto`: ordinary Claude Code requests force
thinking off so normal output is clean, while client-requested thinking or
`--effort` can pass through to DeepSeek.

Thinking policy:

- `DEEPSEEK_THINKING=auto`: default; disable thinking unless the client requests
  thinking or `output_config.effort`.
- `DEEPSEEK_THINKING=disabled`: force thinking off for all requests.
- `DEEPSEEK_THINKING=enabled`: enable thinking when the client does not specify
  it.

Existing environment model mapping values are used only to seed an empty
database. Runtime routing reads SQLite for every request, so admin changes take
effect without restarting the gateway. Enabled adapters are dispatched in
priority order with round-robin rotation across the configured set. DeepSeek's
base URL and upstream protocol are code constants; users configure each
adapter's key and mapping.

## Verification

```sh
cargo test
npm --prefix admin-ui run build
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
