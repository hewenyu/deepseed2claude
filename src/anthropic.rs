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
use crate::{AppState, Error, upstream};

const SUPPORTED_STOP_REASONS: &[&str] = &[
    "end_turn",
    "max_tokens",
    "stop_sequence",
    "tool_use",
    "refusal",
];

pub async fn healthz() -> &'static str {
    "ok"
}

pub async fn models(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "data": [
            model("claude-opus-4-1", "Claude Opus mapped to DeepSeek V4 Pro"),
            model("claude-sonnet-4-5", "Claude Sonnet mapped to DeepSeek V4 Flash"),
            model("claude-3-5-haiku", "Claude Haiku mapped to DeepSeek V4 Flash"),
            model("deepseek-v4-pro", "DeepSeek V4 Pro"),
            model("deepseek-v4-flash", "DeepSeek V4 Flash")
        ],
        "has_more": false,
        "first_id": "claude-opus-4-1",
        "last_id": state.config.default_deepseek_model
    }))
}

pub async fn messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Error> {
    require_client_auth(&headers)?;

    let mut request_body: Value = serde_json::from_slice(&body)?;
    let requested_model = patch_request(&state, &mut request_body)?;
    let is_stream = request_body
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let upstream_response = upstream::send_messages(&state, &headers, request_body).await?;

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
    require_client_auth(&headers)?;

    let mut request_body: Value = serde_json::from_slice(&body)?;
    patch_request(&state, &mut request_body)?;

    let upstream_response = upstream::send_count_tokens(&state, &headers, request_body).await?;
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

fn require_client_auth(headers: &HeaderMap) -> Result<(), Error> {
    let has_x_api_key = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let has_bearer = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_ascii_lowercase().starts_with("bearer "))
        .unwrap_or(false);

    if has_x_api_key || has_bearer {
        Ok(())
    } else {
        Err(Error::Authentication(
            "missing x-api-key or authorization bearer token".to_owned(),
        ))
    }
}

fn patch_request(state: &AppState, body: &mut Value) -> Result<String, Error> {
    let requested_model = {
        let object = body.as_object_mut().ok_or_else(|| {
            Error::InvalidRequest("request body must be a JSON object".to_owned())
        })?;

        let requested_model = object
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::InvalidRequest("model is required".to_owned()))?
            .to_owned();
        let upstream_model = state.config.map_model(&requested_model);
        object.insert("model".to_owned(), Value::String(upstream_model.clone()));

        if !object.contains_key("messages") {
            return Err(Error::InvalidRequest("messages is required".to_owned()));
        }

        debug!(%requested_model, %upstream_model, "patched anthropic request model");
        requested_model
    };

    reject_unsupported_request_content(body)?;

    let object = body
        .as_object_mut()
        .ok_or_else(|| Error::InvalidRequest("request body must be a JSON object".to_owned()))?;
    if let Some(thinking) = &state.config.deepseek_thinking {
        if thinking == "disabled" {
            object.insert("thinking".to_owned(), json!({ "type": thinking }));
            object.remove("output_config");
        } else {
            object
                .entry("thinking".to_owned())
                .or_insert_with(|| json!({ "type": thinking }));
        }

        if let Some(effort) = &state.config.deepseek_reasoning_effort
            && (thinking == "enabled" || thinking == "auto")
        {
            object
                .entry("output_config".to_owned())
                .or_insert_with(|| json!({ "effort": normalize_effort(effort) }));
        }
    }

    Ok(requested_model)
}

fn normalize_effort(effort: &str) -> &'static str {
    match effort {
        "max" | "xhigh" => "max",
        _ => "high",
    }
}

fn reject_unsupported_request_content(value: &Value) -> Result<(), Error> {
    if let Some(system) = value.get("system") {
        validate_content_value(system, "system")?;
    }

    let messages = value
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::InvalidRequest("messages must be an array".to_owned()))?;

    for (message_index, message) in messages.iter().enumerate() {
        let content = message.get("content").ok_or_else(|| {
            Error::InvalidRequest(format!("messages[{message_index}].content is required"))
        })?;
        validate_content_value(content, &format!("messages[{message_index}].content"))?;
    }

    Ok(())
}

fn validate_content_value(content: &Value, path: &str) -> Result<(), Error> {
    match content {
        Value::String(_) | Value::Null => Ok(()),
        Value::Array(blocks) => {
            for (index, block) in blocks.iter().enumerate() {
                validate_content_block(block, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        _ => Err(Error::InvalidRequest(format!(
            "{path} must be a string or content block array"
        ))),
    }
}

fn validate_content_block(block: &Value, path: &str) -> Result<(), Error> {
    let block_type = block
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidRequest(format!("{path}.type is required")))?;

    match block_type {
        "text" | "tool_use" | "thinking" => Ok(()),
        "tool_result" => {
            if let Some(content) = block.get("content") {
                validate_content_value(content, &format!("{path}.content"))?;
            }
            Ok(())
        }
        unsupported => Err(Error::InvalidRequest(format!(
            "unsupported content block at {path}: {unsupported}"
        ))),
    }
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

    use crate::anthropic::{
        patch_message_response, patch_request, reject_unsupported_request_content,
    };
    use crate::{AppState, Config};
    use std::sync::Arc;

    fn state() -> AppState {
        AppState {
            config: Arc::new(Config::for_test("http://upstream".to_owned())),
            client: reqwest::Client::new(),
        }
    }

    #[test]
    fn patch_request_maps_model_and_disables_thinking_by_default() {
        let state = state();
        let mut body = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "messages": [{ "role": "user", "content": "hello" }]
        });

        let requested = patch_request(&state, &mut body).unwrap();

        assert_eq!(requested, "claude-sonnet-4-5");
        assert_eq!(body["model"], "deepseek-v4-flash");
        assert_eq!(body["thinking"], json!({ "type": "disabled" }));
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn patch_request_can_enable_thinking_when_configured() {
        let mut config = Config::for_test("http://upstream".to_owned());
        config.deepseek_thinking = Some("enabled".to_owned());
        let state = AppState {
            config: Arc::new(config),
            client: reqwest::Client::new(),
        };
        let mut body = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "messages": [{ "role": "user", "content": "hello" }]
        });

        patch_request(&state, &mut body).unwrap();

        assert_eq!(body["thinking"], json!({ "type": "enabled" }));
        assert_eq!(body["output_config"], json!({ "effort": "high" }));
    }

    #[test]
    fn patch_request_forces_disabled_thinking_when_configured() {
        let state = state();
        let mut body = json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "thinking": { "type": "enabled" },
            "output_config": { "effort": "max" },
            "messages": [{ "role": "user", "content": "hello" }]
        });

        patch_request(&state, &mut body).unwrap();

        assert_eq!(body["thinking"], json!({ "type": "disabled" }));
        assert!(body.get("output_config").is_none());
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

        let err = reject_unsupported_request_content(&body).unwrap_err();
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

        reject_unsupported_request_content(&body).unwrap();
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
