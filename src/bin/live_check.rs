use deepseed2claude::Config;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    let config = Config::from_env()?;
    let client = reqwest::Client::new();
    let url = config.messages_url();

    let non_stream = with_thinking(
        &config,
        json!({
        "model": "deepseek-v4-flash",
        "max_tokens": 32,
        "messages": [{ "role": "user", "content": "Reply with exactly: pong" }]
        }),
    );

    let response = client
        .post(&url)
        .header("x-api-key", &config.deepseek_api_key)
        .header("anthropic-version", "2023-06-01")
        .header(CONTENT_TYPE, "application/json")
        .json(&non_stream)
        .send()
        .await?;

    let status = response.status();
    let body: Value = response.json().await?;
    if !status.is_success() {
        return Err(format!("non-stream request failed with {status}: {body}").into());
    }
    assert_eq!(body["type"], "message");
    assert!(body["content"].as_array().is_some());
    assert!(body["usage"]["input_tokens"].is_number());
    println!(
        "non-stream ok: stop_reason={:?}, content_blocks={}",
        body["stop_reason"],
        body["content"].as_array().map(Vec::len).unwrap_or_default()
    );

    let stream = with_thinking(
        &config,
        json!({
        "model": "deepseek-v4-flash",
        "max_tokens": 32,
        "stream": true,
        "messages": [{ "role": "user", "content": "Reply with exactly: pong" }]
        }),
    );

    let response = client
        .post(&url)
        .header("x-api-key", &config.deepseek_api_key)
        .header("anthropic-version", "2023-06-01")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "text/event-stream")
        .json(&stream)
        .send()
        .await?;

    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(format!("stream request failed with {status}: {text}").into());
    }
    for event in [
        "event: message_start",
        "event: content_block_start",
        "event: content_block_delta",
        "event: content_block_stop",
        "event: message_delta",
        "event: message_stop",
    ] {
        if !text.contains(event) {
            return Err(format!("stream response missing {event}: {text}").into());
        }
    }
    println!("stream ok: bytes={}", text.len());

    let tool_request = with_thinking(
        &config,
        json!({
        "model": "deepseek-v4-flash",
        "max_tokens": 256,
        "tools": [{
            "name": "get_project_name",
            "description": "Return the current project name",
            "input_schema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }],
        "tool_choice": { "type": "tool", "name": "get_project_name" },
        "messages": [{ "role": "user", "content": "Use the tool to get the project name, then stop." }]
        }),
    );

    let response = client
        .post(&url)
        .header("x-api-key", &config.deepseek_api_key)
        .header("anthropic-version", "2023-06-01")
        .header(CONTENT_TYPE, "application/json")
        .json(&tool_request)
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    if !status.is_success() {
        return Err(format!("tool request failed with {status}: {body}").into());
    }
    assert_eq!(body["stop_reason"], "tool_use");
    assert_eq!(body["content"][0]["type"], "tool_use");
    println!("tool-use ok: {}", body["content"][0]["name"]);

    let count_tokens = with_thinking(
        &config,
        json!({
            "model": "deepseek-v4-flash",
            "messages": [{ "role": "user", "content": "hello" }]
        }),
    );
    let response = client
        .post(config.count_tokens_url())
        .header("x-api-key", &config.deepseek_api_key)
        .header("anthropic-version", "2023-06-01")
        .header(CONTENT_TYPE, "application/json")
        .json(&count_tokens)
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    if !status.is_success() {
        return Err(format!("count_tokens request failed with {status}: {body}").into());
    }
    assert!(body["input_tokens"].is_number());
    println!("count-tokens ok: input_tokens={}", body["input_tokens"]);

    Ok(())
}

fn with_thinking(config: &Config, mut body: Value) -> Value {
    if let Some(object) = body.as_object_mut()
        && let Some(thinking) = &config.deepseek_thinking
    {
        match thinking.as_str() {
            "enabled" => {
                object.insert("thinking".to_owned(), json!({ "type": "enabled" }));
                if let Some(effort) = &config.deepseek_reasoning_effort {
                    object.insert("output_config".to_owned(), json!({ "effort": effort }));
                }
            }
            "disabled" | "auto" => {
                object.insert("thinking".to_owned(), json!({ "type": "disabled" }));
                object.remove("output_config");
            }
            _ => {}
        }
    }
    body
}
