use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{Map, Value, json};
use tracing::{debug, warn};

use crate::error::anthropic_error_response;
use crate::store::UpstreamSelection;
use crate::{AppState, Error, upstream};

const SUPPORTED_STOP_REASONS: &[&str] = &[
    "end_turn",
    "max_tokens",
    "stop_sequence",
    "tool_use",
    "refusal",
];
const SUPPORTED_REQUEST_FIELDS: &[&str] = &[
    "model",
    "max_tokens",
    "messages",
    "metadata",
    "output_config",
    "stop_sequences",
    "stream",
    "system",
    "temperature",
    "thinking",
    "tool_choice",
    "tools",
    "top_p",
];
const IGNORED_REQUEST_FIELDS: &[&str] = &["container", "mcp_servers", "service_tier", "top_k"];

pub async fn healthz() -> &'static str {
    "ok"
}

pub async fn models(State(state): State<AppState>) -> Result<Json<Value>, Error> {
    let admin_state = state.store.admin_state().await?;
    let mut models = vec![
        model("claude-opus-4-1", "Claude Opus mapped by active ADP"),
        model("claude-sonnet-4-5", "Claude Sonnet mapped by active ADP"),
        model("claude-3-5-haiku", "Claude Haiku mapped by active ADP"),
    ];
    for adapter in admin_state.adapters.iter().filter(|adapter| adapter.enabled) {
        for id in [
            adapter.default_model.as_str(),
            adapter.opus_model.as_str(),
            adapter.sonnet_model.as_str(),
            adapter.haiku_model.as_str(),
        ] {
            if !models.iter().any(|model| model["id"] == id) {
                models.push(model(id, &format!("{} upstream model", adapter.name)));
            }
        }
    }
    let first_id = models
        .first()
        .and_then(|model| model.get("id"))
        .cloned()
        .unwrap_or_else(|| json!("claude-opus-4-1"));
    let last_id = models
        .last()
        .and_then(|model| model.get("id"))
        .cloned()
        .unwrap_or_else(|| json!("claude-3-5-haiku"));

    Ok(Json(json!({
        "data": models,
        "has_more": false,
        "first_id": first_id,
        "last_id": last_id
    })))
}

pub async fn messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Error> {
    require_client_auth(&state, &headers).await?;

    let mut request_body: Value = serde_json::from_slice(&body)?;
    let selection = state.store.select_upstream(state.next_dispatch_slot()).await?;
    let requested_model = patch_request(&selection, &mut request_body)?;
    let is_stream = request_body
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let upstream_response = upstream::send_messages(&state, &selection, &headers, request_body).await?;

    if !upstream_response.status().is_success() {
        return Ok(normalize_upstream_error(upstream_response).await);
    }

    if is_stream {
        Ok(stream_response(upstream_response, requested_model))
    } else {
        normalize_message_response(upstream_response, &requested_model).await
    }
}

pub async fn count_tokens(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Error> {
    require_client_auth(&state, &headers).await?;

    let mut request_body: Value = serde_json::from_slice(&body)?;
    let selection = state.store.select_upstream(state.next_dispatch_slot()).await?;
    patch_request(&selection, &mut request_body)?;

    let upstream_response =
        upstream::send_count_tokens(&state, &selection, &headers, request_body).await?;
    if !upstream_response.status().is_success() {
        return Ok(normalize_upstream_error(upstream_response).await);
    }

    normalize_count_tokens_response(upstream_response).await
}

fn model(id: &str, display_name: &str) -> Value {
    json!({
        "id": id,
        "type": "model",
        "display_name": display_name
    })
}

async fn require_client_auth(state: &AppState, headers: &HeaderMap) -> Result<(), Error> {
    let api_key = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().strip_prefix("Bearer "))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        });

    let Some(api_key) = api_key else {
        return Err(Error::Authentication(
            "missing x-api-key or authorization bearer token".to_owned(),
        ));
    };

    if state.store.authenticate_client_key(&api_key).await? {
        Ok(())
    } else {
        Err(Error::Authentication("invalid client API key".to_owned()))
    }
}

fn patch_request(selection: &UpstreamSelection, body: &mut Value) -> Result<String, Error> {
    let requested_model = {
        let object = body.as_object_mut().ok_or_else(|| {
            Error::InvalidRequest("request body must be a JSON object".to_owned())
        })?;

        let requested_model = object
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::InvalidRequest("model is required".to_owned()))?
            .to_owned();
        let upstream_model = selection.adapter.map_model(&requested_model);
        object.insert("model".to_owned(), Value::String(upstream_model.clone()));

        if !object.contains_key("messages") {
            return Err(Error::InvalidRequest("messages is required".to_owned()));
        }

        debug!(%requested_model, %upstream_model, "patched anthropic request model");
        requested_model
    };

    let object = body
        .as_object_mut()
        .ok_or_else(|| Error::InvalidRequest("request body must be a JSON object".to_owned()))?;
    normalize_request_for_deepseek(object)?;
    patch_thinking_policy(selection, object);

    Ok(requested_model)
}

fn normalize_request_for_deepseek(object: &mut Map<String, Value>) -> Result<(), Error> {
    strip_unsupported_request_fields(object);
    normalize_metadata(object);
    normalize_tools(object)?;
    normalize_tool_choice(object)?;
    normalize_request_content(object)
}

fn strip_unsupported_request_fields(object: &mut Map<String, Value>) {
    object.retain(|field, _| {
        SUPPORTED_REQUEST_FIELDS.contains(&field.as_str())
            || IGNORED_REQUEST_FIELDS.contains(&field.as_str())
    });
}

fn normalize_metadata(object: &mut Map<String, Value>) {
    let Some(metadata) = object.get_mut("metadata").and_then(Value::as_object_mut) else {
        return;
    };
    metadata.retain(|field, _| field == "user_id");
    if metadata.is_empty() {
        object.remove("metadata");
    }
}

fn normalize_tools(object: &mut Map<String, Value>) -> Result<(), Error> {
    let Some(tools) = object.get_mut("tools") else {
        return Ok(());
    };
    let tools = tools
        .as_array_mut()
        .ok_or_else(|| Error::InvalidRequest("tools must be an array".to_owned()))?;

    for (index, tool) in tools.iter_mut().enumerate() {
        let tool = tool.as_object_mut().ok_or_else(|| {
            Error::InvalidRequest(format!("tools[{index}] must be a JSON object"))
        })?;
        for required in ["name", "input_schema"] {
            if !tool.contains_key(required) {
                return Err(Error::InvalidRequest(format!(
                    "tools[{index}].{required} is required"
                )));
            }
        }
        tool.retain(|field, _| matches!(field.as_str(), "name" | "description" | "input_schema"));
    }

    Ok(())
}

fn normalize_tool_choice(object: &mut Map<String, Value>) -> Result<(), Error> {
    let Some(tool_choice) = object.get_mut("tool_choice") else {
        return Ok(());
    };
    let tool_choice = tool_choice
        .as_object_mut()
        .ok_or_else(|| Error::InvalidRequest("tool_choice must be an object".to_owned()))?;
    let choice_type = tool_choice
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidRequest("tool_choice.type is required".to_owned()))?;

    match choice_type {
        "none" | "auto" | "any" => {
            tool_choice.retain(|field, _| field == "type");
            Ok(())
        }
        "tool" => {
            if tool_choice.get("name").and_then(Value::as_str).is_none() {
                return Err(Error::InvalidRequest(
                    "tool_choice.name is required when type is tool".to_owned(),
                ));
            }
            tool_choice.retain(|field, _| matches!(field.as_str(), "type" | "name"));
            Ok(())
        }
        unsupported => Err(Error::InvalidRequest(format!(
            "unsupported tool_choice.type: {unsupported}"
        ))),
    }
}

fn patch_thinking_policy(selection: &UpstreamSelection, object: &mut Map<String, Value>) {
    let mode = selection.adapter.thinking.as_deref().unwrap_or("auto");

    match mode {
        "disabled" => {
            object.insert("thinking".to_owned(), json!({ "type": "disabled" }));
            object.remove("output_config");
        }
        "enabled" => {
            object
                .entry("thinking".to_owned())
                .or_insert_with(|| json!({ "type": "enabled" }));
            ensure_output_effort(selection, object);
        }
        "auto" => {
            if has_client_thinking_enabled(object) {
                ensure_output_effort(selection, object);
            } else if object.contains_key("output_config") {
                object
                    .entry("thinking".to_owned())
                    .or_insert_with(|| json!({ "type": "enabled" }));
                normalize_existing_effort(object);
            } else {
                object.insert("thinking".to_owned(), json!({ "type": "disabled" }));
            }
        }
        _ => {
            object.insert("thinking".to_owned(), json!({ "type": "disabled" }));
            object.remove("output_config");
        }
    }
}

fn has_client_thinking_enabled(object: &Map<String, Value>) -> bool {
    object
        .get("thinking")
        .and_then(Value::as_object)
        .and_then(|thinking| thinking.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|kind| kind == "enabled" || kind == "auto")
}

fn ensure_output_effort(selection: &UpstreamSelection, object: &mut Map<String, Value>) {
    if let Some(effort) = &selection.adapter.reasoning_effort {
        object
            .entry("output_config".to_owned())
            .or_insert_with(|| json!({ "effort": normalize_effort(effort) }));
    }
    normalize_existing_effort(object);
}

fn normalize_existing_effort(object: &mut Map<String, Value>) {
    if let Some(output_config) = object
        .get_mut("output_config")
        .and_then(Value::as_object_mut)
        && let Some(effort) = output_config.get("effort").and_then(Value::as_str)
    {
        output_config.insert(
            "effort".to_owned(),
            Value::String(normalize_effort(effort).to_owned()),
        );
    }
}

fn normalize_effort(effort: &str) -> &'static str {
    match effort {
        "max" | "xhigh" => "max",
        _ => "high",
    }
}

fn normalize_request_content(object: &mut Map<String, Value>) -> Result<(), Error> {
    if let Some(system) = object.get_mut("system") {
        normalize_content_value(system, "system")?;
    }

    let messages = object
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| Error::InvalidRequest("messages must be an array".to_owned()))?;

    for (message_index, message) in messages.iter().enumerate() {
        validate_message_role(message, message_index)?;
    }

    for (message_index, message) in messages.iter_mut().enumerate() {
        let message = message.as_object_mut().ok_or_else(|| {
            Error::InvalidRequest(format!("messages[{message_index}] must be a JSON object"))
        })?;
        let content = message.get_mut("content").ok_or_else(|| {
            Error::InvalidRequest(format!("messages[{message_index}].content is required"))
        })?;
        normalize_content_value(content, &format!("messages[{message_index}].content"))?;
    }

    Ok(())
}

fn validate_message_role(message: &Value, index: usize) -> Result<(), Error> {
    match message.get("role").and_then(Value::as_str) {
        Some("user" | "assistant") => Ok(()),
        Some(role) => Err(Error::InvalidRequest(format!(
            "messages[{index}].role is unsupported: {role}"
        ))),
        None => Err(Error::InvalidRequest(format!(
            "messages[{index}].role is required"
        ))),
    }
}

fn normalize_content_value(content: &mut Value, path: &str) -> Result<(), Error> {
    match content {
        Value::String(_) | Value::Null => Ok(()),
        Value::Array(blocks) => {
            for (index, block) in blocks.iter_mut().enumerate() {
                normalize_content_block(block, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        _ => Err(Error::InvalidRequest(format!(
            "{path} must be a string or content block array"
        ))),
    }
}

fn normalize_content_block(block: &mut Value, path: &str) -> Result<(), Error> {
    let block_type = block
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::InvalidRequest(format!("{path}.type is required")))?;

    match block_type.as_str() {
        "text" => retain_block_fields(block, &["type", "text"]),
        "tool_use" => retain_block_fields(block, &["type", "id", "name", "input"]),
        "thinking" => retain_block_fields(block, &["type", "thinking", "signature"]),
        "tool_result" => {
            retain_block_fields(block, &["type", "tool_use_id", "content", "is_error"])?;
            if let Some(content) = block.get_mut("content") {
                normalize_content_value(content, &format!("{path}.content"))?;
            }
            Ok(())
        }
        unsupported => Err(Error::InvalidRequest(format!(
            "unsupported content block at {path}: {unsupported}"
        ))),
    }
}

fn retain_block_fields(block: &mut Value, supported_fields: &[&str]) -> Result<(), Error> {
    let object = block
        .as_object_mut()
        .ok_or_else(|| Error::InvalidRequest("content block must be an object".to_owned()))?;
    object.retain(|field, _| supported_fields.contains(&field.as_str()));
    Ok(())
}

async fn normalize_message_response(
    upstream_response: reqwest::Response,
    requested_model: &str,
) -> Result<Response, Error> {
    let status = upstream_response.status();
    let mut headers = HeaderMap::new();
    copy_header(upstream_response.headers(), &mut headers, CONTENT_TYPE);
    let text = upstream_response.text().await?;
    let mut value: Value = serde_json::from_str(&text).map_err(|err| {
        Error::Upstream(format!("upstream returned invalid JSON response: {err}"))
    })?;

    patch_message_response(&mut value, requested_model)?;

    let body = serde_json::to_vec(&value)?;
    let mut response = Response::builder().status(status);
    for (name, value) in headers.iter() {
        response = response.header(name, value);
    }
    Ok(response
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("valid response"))
}

async fn normalize_count_tokens_response(
    upstream_response: reqwest::Response,
) -> Result<Response, Error> {
    let status = upstream_response.status();
    let text = upstream_response.text().await?;
    let value: Value = serde_json::from_str(&text).map_err(|err| {
        Error::Upstream(format!(
            "upstream returned invalid count_tokens JSON response: {err}"
        ))
    })?;

    validate_count_tokens_response(&value)?;

    let body = serde_json::to_vec(&value)?;
    Ok(Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("valid response"))
}

fn validate_count_tokens_response(value: &Value) -> Result<(), Error> {
    if value.get("input_tokens").and_then(Value::as_u64).is_some() {
        Ok(())
    } else {
        Err(Error::Upstream(
            "upstream count_tokens response missing numeric input_tokens".to_owned(),
        ))
    }
}

fn patch_message_response(value: &mut Value, requested_model: &str) -> Result<(), Error> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| Error::Upstream("upstream message response must be an object".to_owned()))?;

    require_field_eq(object, "type", "message")?;
    require_field_eq(object, "role", "assistant")?;
    validate_response_content(object.get("content"))?;
    validate_stop_reason(object.get("stop_reason"))?;

    object.insert(
        "model".to_owned(),
        Value::String(requested_model.to_owned()),
    );
    Ok(())
}

fn require_field_eq(object: &Map<String, Value>, field: &str, expected: &str) -> Result<(), Error> {
    match object.get(field).and_then(Value::as_str) {
        Some(actual) if actual == expected => Ok(()),
        Some(actual) => Err(Error::Upstream(format!(
            "upstream response field {field} was {actual:?}, expected {expected:?}"
        ))),
        None => Err(Error::Upstream(format!(
            "upstream response missing required field {field}"
        ))),
    }
}

fn validate_response_content(content: Option<&Value>) -> Result<(), Error> {
    let blocks = content
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Upstream("upstream response content must be an array".to_owned()))?;
    for (index, block) in blocks.iter().enumerate() {
        let block_type = block.get("type").and_then(Value::as_str).ok_or_else(|| {
            Error::Upstream(format!(
                "upstream response content[{index}].type is required"
            ))
        })?;
        match block_type {
            "text" | "tool_use" | "thinking" => {}
            unsupported => {
                return Err(Error::Upstream(format!(
                    "upstream response contains unsupported content block: {unsupported}"
                )));
            }
        }
    }
    Ok(())
}

fn validate_stop_reason(stop_reason: Option<&Value>) -> Result<(), Error> {
    match stop_reason {
        Some(Value::String(reason)) if SUPPORTED_STOP_REASONS.contains(&reason.as_str()) => Ok(()),
        Some(Value::Null) | None => Ok(()),
        Some(Value::String(reason)) => Err(Error::Upstream(format!(
            "upstream response stop_reason is unsupported: {reason}"
        ))),
        Some(_) => Err(Error::Upstream(
            "upstream response stop_reason must be a string or null".to_owned(),
        )),
    }
}

fn stream_response(upstream_response: reqwest::Response, requested_model: String) -> Response {
    let mut response = Response::builder().status(StatusCode::OK);
    response = response.header(CONTENT_TYPE, "text/event-stream");
    response = response.header(CACHE_CONTROL, "no-cache");

    let stream = patch_sse_stream(upstream_response, requested_model);

    response
        .body(Body::from_stream(stream))
        .expect("valid stream response")
}

fn patch_sse_stream(
    upstream_response: reqwest::Response,
    requested_model: String,
) -> impl futures_util::Stream<Item = Result<Bytes, std::io::Error>> {
    async_stream::try_stream! {
        let mut stream = upstream_response.bytes_stream();
        let mut pending = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| {
                warn!(%err, "upstream stream failed");
                std::io::Error::other(err.to_string())
            })?;
            pending.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(frame_end) = pending.find("\n\n") {
                let frame = pending[..frame_end + 2].to_owned();
                pending = pending[frame_end + 2..].to_owned();
                yield Bytes::from(patch_sse_frame(&frame, &requested_model));
            }
        }

        if !pending.is_empty() {
            yield Bytes::from(patch_sse_frame(&pending, &requested_model));
        }
    }
}

fn patch_sse_frame(frame: &str, requested_model: &str) -> String {
    if !frame.contains("event: message_start") {
        return frame.to_owned();
    }

    let mut patched = Vec::new();
    for line in frame.lines() {
        if let Some(data) = line.strip_prefix("data: ")
            && let Ok(mut value) = serde_json::from_str::<Value>(data)
            && value.get("type").and_then(Value::as_str) == Some("message_start")
        {
            if let Some(message) = value.get_mut("message").and_then(Value::as_object_mut) {
                message.insert(
                    "model".to_owned(),
                    Value::String(requested_model.to_owned()),
                );
            }
            patched.push(format!("data: {value}"));
            continue;
        }
        patched.push(line.to_owned());
    }

    if frame.ends_with("\n\n") {
        format!("{}\n\n", patched.join("\n"))
    } else {
        patched.join("\n")
    }
}

async fn normalize_upstream_error(upstream_response: reqwest::Response) -> Response {
    let status = upstream_response.status();
    let body = upstream_response
        .text()
        .await
        .unwrap_or_else(|err| format!("failed to read upstream error body: {err}"));

    let (client_status, error_type) = match status {
        StatusCode::TOO_MANY_REQUESTS => (StatusCode::TOO_MANY_REQUESTS, "rate_limit_error"),
        StatusCode::BAD_REQUEST => (StatusCode::BAD_REQUEST, "invalid_request_error"),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => (StatusCode::BAD_GATEWAY, "api_error"),
        _ if status.is_server_error() => (StatusCode::BAD_GATEWAY, "api_error"),
        _ => (status, "api_error"),
    };

    anthropic_error_response(
        client_status,
        error_type,
        format!("DeepSeek upstream returned {status}: {body}"),
    )
}

fn copy_header(source: &HeaderMap, target: &mut HeaderMap, name: axum::http::header::HeaderName) {
    if let Some(value) = source.get(&name)
        && let Ok(value) = HeaderValue::from_bytes(value.as_bytes())
    {
        target.insert(name, value);
    }
}

#[derive(Serialize)]
struct _CompileAssert;

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use serde_json::json;

    use crate::anthropic::{normalize_request_content, patch_message_response, patch_request};
    use crate::store::{Adapter, UpstreamSelection};

    fn selection() -> UpstreamSelection {
        UpstreamSelection {
            adapter: Adapter {
                id: 1,
                name: "DeepSeek".to_owned(),
                kind: "deepseek".to_owned(),
                base_url_override: None,
                api_key: "test-deepseek-key".to_owned(),
                enabled: true,
                priority: 10,
                default_model: "deepseek-v4-flash".to_owned(),
                opus_model: "deepseek-v4-pro".to_owned(),
                sonnet_model: "deepseek-v4-flash".to_owned(),
                haiku_model: "deepseek-v4-flash".to_owned(),
                thinking: Some("auto".to_owned()),
                reasoning_effort: Some("high".to_owned()),
            },
        }
    }

    #[test]
    fn patch_request_maps_model_and_disables_thinking_by_default() {
        let selection = selection();
        let mut body = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "messages": [{ "role": "user", "content": "hello" }]
        });

        let requested = patch_request(&selection, &mut body).unwrap();

        assert_eq!(requested, "claude-sonnet-4-5");
        assert_eq!(body["model"], "deepseek-v4-flash");
        assert_eq!(body["thinking"], json!({ "type": "disabled" }));
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn patch_request_can_enable_thinking_when_configured() {
        let mut selection = selection();
        selection.adapter.thinking = Some("enabled".to_owned());
        let mut body = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "messages": [{ "role": "user", "content": "hello" }]
        });

        patch_request(&selection, &mut body).unwrap();

        assert_eq!(body["thinking"], json!({ "type": "enabled" }));
        assert_eq!(body["output_config"], json!({ "effort": "high" }));
    }

    #[test]
    fn patch_request_forces_disabled_thinking_when_configured() {
        let mut selection = selection();
        selection.adapter.thinking = Some("disabled".to_owned());
        let mut body = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "thinking": { "type": "enabled" },
            "output_config": { "effort": "max" },
            "messages": [{ "role": "user", "content": "hello" }]
        });

        patch_request(&selection, &mut body).unwrap();

        assert_eq!(body["thinking"], json!({ "type": "disabled" }));
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn patch_request_passes_client_requested_thinking_in_auto_mode() {
        let selection = selection();
        let mut body = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "thinking": { "type": "enabled" },
            "messages": [{ "role": "user", "content": "hello" }]
        });

        patch_request(&selection, &mut body).unwrap();

        assert_eq!(body["thinking"], json!({ "type": "enabled" }));
        assert_eq!(body["output_config"], json!({ "effort": "high" }));
    }

    #[test]
    fn patch_request_enables_thinking_when_client_sends_effort_in_auto_mode() {
        let selection = selection();
        let mut body = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "output_config": { "effort": "xhigh" },
            "messages": [{ "role": "user", "content": "hello" }]
        });

        patch_request(&selection, &mut body).unwrap();

        assert_eq!(body["thinking"], json!({ "type": "enabled" }));
        assert_eq!(body["output_config"], json!({ "effort": "max" }));
    }

    #[test]
    fn patch_request_strips_unsupported_and_ignored_deepseek_fields() {
        let selection = selection();
        let mut body = json!({
            "model": "claude-opus-4-7",
            "max_tokens": 1024,
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "hello",
                    "citations": [],
                    "cache_control": { "type": "ephemeral" }
                }]
            }],
            "metadata": {
                "user_id": "user-1",
                "trace_id": "drop-me"
            },
            "tools": [{
                "name": "lookup",
                "description": "Look up a value",
                "input_schema": { "type": "object" },
                "cache_control": { "type": "ephemeral" }
            }],
            "tool_choice": {
                "type": "tool",
                "name": "lookup",
                "disable_parallel_tool_use": true
            },
            "container": "ignored-by-deepseek",
            "mcp_servers": [],
            "service_tier": "auto",
            "top_k": 10,
            "unknown_beta_field": true
        });

        patch_request(&selection, &mut body).unwrap();

        assert_eq!(body["model"], "deepseek-v4-pro");
        assert_eq!(body["metadata"], json!({ "user_id": "user-1" }));
        assert_eq!(
            body["messages"][0]["content"][0],
            json!({ "type": "text", "text": "hello" })
        );
        assert_eq!(
            body["tools"][0],
            json!({
                "name": "lookup",
                "description": "Look up a value",
                "input_schema": { "type": "object" }
            })
        );
        assert_eq!(
            body["tool_choice"],
            json!({ "type": "tool", "name": "lookup" })
        );
        assert!(body.get("unknown_beta_field").is_none());
        assert!(body.get("container").is_some());
        assert!(body.get("mcp_servers").is_some());
        assert!(body.get("service_tier").is_some());
        assert!(body.get("top_k").is_some());
    }

    #[test]
    fn patch_request_rejects_unsupported_tool_choice() {
        let selection = selection();
        let mut body = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "messages": [{ "role": "user", "content": "hello" }],
            "tool_choice": { "type": "computer" }
        });

        let err = patch_request(&selection, &mut body).unwrap_err();

        assert!(err.to_string().contains("unsupported tool_choice.type"));
    }

    #[test]
    fn patch_request_rejects_unsupported_message_roles() {
        let selection = selection();
        let mut body = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "messages": [{ "role": "system", "content": "hello" }]
        });

        let err = patch_request(&selection, &mut body).unwrap_err();

        assert!(err.to_string().contains("role is unsupported"));
    }

    #[test]
    fn patch_sse_frame_echoes_requested_model_in_message_start() {
        let frame = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"deepseek-v4-pro\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n"
        );

        let patched = super::patch_sse_frame(frame, "claude-sonnet-4-5");

        assert!(patched.contains("\"model\":\"claude-sonnet-4-5\""));
    }

    #[test]
    fn rejects_unsupported_request_blocks() {
        let body = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "messages": [{
                "role": "user",
                "content": [{ "type": "image", "source": {} }]
            }]
        });

        let mut body = body;
        let err = normalize_request_content(body.as_object_mut().unwrap()).unwrap_err();
        assert!(err.to_string().contains("unsupported content block"));
    }

    #[test]
    fn preserves_tool_result_text_content() {
        let body = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_1",
                    "content": [{ "type": "text", "text": "done" }]
                }]
            }]
        });

        let mut body = body;
        normalize_request_content(body.as_object_mut().unwrap()).unwrap();
    }

    #[test]
    fn patch_response_echoes_requested_model_and_validates_shape() {
        let mut body = json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "text", "text": "hello" }],
            "model": "deepseek-v4-flash",
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": { "input_tokens": 1, "output_tokens": 1 }
        });

        patch_message_response(&mut body, "claude-sonnet-4-5").unwrap();

        assert_eq!(body["model"], "claude-sonnet-4-5");
    }

    #[tokio::test]
    async fn auth_error_is_anthropic_shaped() {
        let err = crate::Error::Authentication("missing key".to_owned()).into_response();
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
        let bytes = to_bytes(err.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["type"], "error");
        assert_eq!(body["error"]["type"], "authentication_error");
    }
}
