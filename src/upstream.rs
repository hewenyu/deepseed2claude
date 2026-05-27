use axum::http::HeaderMap;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::Value;

use crate::store::UpstreamSelection;
use crate::{AppState, Error};

const ANTHROPIC_VERSION: &str = "anthropic-version";
const ANTHROPIC_BETA: &str = "anthropic-beta";
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";

pub async fn send_messages(
    state: &AppState,
    selection: &UpstreamSelection,
    client_headers: &HeaderMap,
    body: Value,
) -> Result<reqwest::Response, Error> {
    send_anthropic_request(
        state,
        selection,
        client_headers,
        selection.messages_url()?,
        body,
    )
    .await
}

pub async fn send_count_tokens(
    state: &AppState,
    selection: &UpstreamSelection,
    client_headers: &HeaderMap,
    body: Value,
) -> Result<reqwest::Response, Error> {
    send_anthropic_request(
        state,
        selection,
        client_headers,
        selection.count_tokens_url()?,
        body,
    )
    .await
}

async fn send_anthropic_request(
    state: &AppState,
    selection: &UpstreamSelection,
    client_headers: &HeaderMap,
    url: String,
    body: Value,
) -> Result<reqwest::Response, Error> {
    let mut request = state
        .client
        .post(url)
        .header("x-api-key", selection.api_key())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .json(&body);

    if let Some(version) = client_headers
        .get(ANTHROPIC_VERSION)
        .and_then(|value| value.to_str().ok())
    {
        request = request.header(ANTHROPIC_VERSION, version);
    } else {
        request = request.header(ANTHROPIC_VERSION, DEFAULT_ANTHROPIC_VERSION);
    }

    if let Some(beta) = client_headers
        .get(ANTHROPIC_BETA)
        .and_then(|value| value.to_str().ok())
    {
        request = request.header(ANTHROPIC_BETA, beta);
    }

    Ok(request.send().await?)
}
