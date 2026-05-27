use std::net::SocketAddr;

use axum::body::{Body, to_bytes};
use axum::http::header::{ACCEPT, CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use deepseed2claude::{Config, test_app};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tower::ServiceExt;

#[tokio::test]
async fn forwards_patched_non_stream_request_and_echoes_client_model() {
    let upstream =
        spawn_upstream(Router::new().route("/v1/messages", post(capture_non_stream))).await;
    let app = test_app(Config::for_test(upstream.url())).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("x-api-key", "test")
                .header("anthropic-version", "2023-06-01")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "model": "claude-sonnet-4-5",
                        "max_tokens": 64,
                        "messages": [{ "role": "user", "content": "hello" }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["model"], "claude-sonnet-4-5");
    assert_eq!(body["content"][0]["text"], "ok");
    assert_eq!(body["usage"]["input_tokens"], 3);
}

#[tokio::test]
async fn accepts_bearer_auth_for_claude_code_token_mode() {
    let upstream =
        spawn_upstream(Router::new().route("/v1/messages", post(capture_non_stream))).await;
    let app = test_app(Config::for_test(upstream.url())).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("authorization", "Bearer test")
                .header("anthropic-version", "2023-06-01")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "model": "claude-sonnet-4-5",
                        "max_tokens": 64,
                        "messages": [{ "role": "user", "content": "hello" }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn sanitizes_request_before_forwarding_to_deepseek() {
    let upstream =
        spawn_upstream(Router::new().route("/v1/messages", post(capture_sanitized_request))).await;
    let app = test_app(Config::for_test(upstream.url())).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("x-api-key", "test")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "model": "claude-opus-4-7",
                        "max_tokens": 64,
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
                        "extra_unsupported_field": true
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["model"], "claude-opus-4-7");
}

#[tokio::test]
async fn passes_streaming_sse_through() {
    let upstream =
        spawn_upstream(Router::new().route("/v1/messages", post(streaming_response))).await;
    let app = test_app(Config::for_test(upstream.url())).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("x-api-key", "test")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "model": "claude-sonnet-4-5",
                        "max_tokens": 64,
                        "stream": true,
                        "messages": [{ "role": "user", "content": "hello" }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).unwrap(),
        "text/event-stream"
    );
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("event: message_start"));
    assert!(body.contains("event: content_block_delta"));
    assert!(body.contains("event: message_stop"));
    assert!(body.contains("\"model\":\"claude-sonnet-4-5\""));
    assert!(body.contains("\"text\":\"hi\""));
}

#[tokio::test]
async fn forwards_count_tokens_requests() {
    let upstream =
        spawn_upstream(Router::new().route("/v1/messages/count_tokens", post(count_tokens))).await;
    let app = test_app(Config::for_test(upstream.url())).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/messages/count_tokens")
                .header("x-api-key", "test")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "model": "claude-opus-4-1",
                        "messages": [{ "role": "user", "content": "hello" }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["input_tokens"], 42);
}

#[tokio::test]
async fn exposes_models_endpoint() {
    let app = test_app(Config::for_test("http://127.0.0.1:9".to_owned())).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert!(body["data"].as_array().unwrap().len() >= 2);
}

#[tokio::test]
async fn admin_can_create_client_key_for_gateway_auth_immediately() {
    let upstream =
        spawn_upstream(Router::new().route("/v1/messages", post(capture_non_stream))).await;
    let app = test_app(Config::for_test(upstream.url())).await.unwrap();

    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/admin/login")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "username": "admin", "password": "password" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/admin/client-keys")
                .header(COOKIE, cookie)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "name": "claude-code",
                        "api_key": "managed-client-key",
                        "enabled": true,
                        "priority": 5
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("x-api-key", "managed-client-key")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "model": "claude-sonnet-4-5",
                        "max_tokens": 64,
                        "messages": [{ "role": "user", "content": "hello" }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn admin_adapter_changes_are_used_without_restart() {
    async fn capture_alt_adapter(headers: HeaderMap, Json(body): Json<Value>) -> Response {
        assert_eq!(
            headers.get("x-api-key").unwrap().to_str().unwrap(),
            "alt-upstream-key"
        );
        assert_eq!(body["model"], "deepseek-v4-pro");
        Json(json!({
            "id": "msg_mock",
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "text", "text": "ok" }],
            "model": "deepseek-v4-pro",
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": { "input_tokens": 3, "output_tokens": 1 }
        }))
        .into_response()
    }

    let upstream = spawn_upstream(Router::new().route("/v1/messages", post(capture_alt_adapter))).await;
    let app = test_app(Config::for_test(upstream.url())).await.unwrap();

    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/admin/login")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "username": "admin", "password": "password" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = login
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();

    let adapter = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/admin/adapters")
                .header(COOKIE, cookie)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "name": "Alt DeepSeek",
                        "kind": "deepseek",
                        "base_url_override": upstream.url(),
                        "api_key": "alt-upstream-key",
                        "enabled": true,
                        "priority": 1,
                        "default_model": "deepseek-v4-flash",
                        "opus_model": "deepseek-v4-pro",
                        "sonnet_model": "deepseek-v4-pro",
                        "haiku_model": "deepseek-v4-flash",
                        "thinking": "disabled",
                        "reasoning_effort": "high"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(adapter.status(), StatusCode::CREATED);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("x-api-key", "test")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "model": "claude-sonnet-4-5",
                        "max_tokens": 64,
                        "messages": [{ "role": "user", "content": "hello" }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn dispatches_across_multiple_adapters() {
    let upstream =
        spawn_upstream(Router::new().route("/v1/messages", post(capture_round_robin_adapter))).await;
    let app = test_app(Config::for_test(upstream.url())).await.unwrap();

    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/admin/login")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "username": "admin", "password": "password" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = login
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();

    let adapter = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/admin/adapters")
                .header(COOKIE, cookie)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "name": "Second DeepSeek",
                        "kind": "deepseek",
                        "base_url_override": upstream.url(),
                        "api_key": "second-upstream-key",
                        "enabled": true,
                        "priority": 10,
                        "default_model": "deepseek-v4-flash",
                        "opus_model": "deepseek-v4-pro",
                        "sonnet_model": "deepseek-v4-flash",
                        "haiku_model": "deepseek-v4-flash",
                        "thinking": "disabled",
                        "reasoning_effort": "high"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(adapter.status(), StatusCode::CREATED);

    let mut seen_keys = Vec::new();
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/messages")
                    .header("x-api-key", "test")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "model": "claude-sonnet-4-5",
                            "max_tokens": 64,
                            "messages": [{ "role": "user", "content": "hello" }]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        seen_keys.push(body["content"][0]["text"].as_str().unwrap().to_owned());
    }
    seen_keys.sort();
    assert_eq!(
        seen_keys,
        vec![
            "second-upstream-key".to_owned(),
            "test-deepseek-key".to_owned()
        ]
    );
}

#[tokio::test]
async fn rejects_unsupported_content_before_upstream() {
    let upstream =
        spawn_upstream(Router::new().route("/v1/messages", post(unexpected_upstream))).await;
    let app = test_app(Config::for_test(upstream.url())).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("x-api-key", "test")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "model": "claude-sonnet-4-5",
                        "max_tokens": 64,
                        "messages": [{
                            "role": "user",
                            "content": [{ "type": "image", "source": {} }]
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unsupported content block")
    );
}

#[tokio::test]
async fn rejects_missing_messages_before_upstream() {
    let upstream =
        spawn_upstream(Router::new().route("/v1/messages", post(unexpected_upstream))).await;
    let app = test_app(Config::for_test(upstream.url())).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("x-api-key", "test")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "model": "claude-sonnet-4-5",
                        "max_tokens": 64
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("messages is required")
    );
}

#[tokio::test]
async fn normalizes_upstream_rate_limit_error() {
    let upstream = spawn_upstream(Router::new().route("/v1/messages", post(rate_limited))).await;
    let app = test_app(Config::for_test(upstream.url())).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("x-api-key", "test")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "model": "claude-sonnet-4-5",
                        "max_tokens": 64,
                        "messages": [{ "role": "user", "content": "hello" }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body = json_body(response).await;
    assert_eq!(body["error"]["type"], "rate_limit_error");
}

#[tokio::test]
async fn normalizes_stream_upstream_errors_as_json_errors() {
    let upstream = spawn_upstream(Router::new().route("/v1/messages", post(rate_limited))).await;
    let app = test_app(Config::for_test(upstream.url())).await.unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("x-api-key", "test")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "model": "claude-sonnet-4-5",
                        "max_tokens": 64,
                        "stream": true,
                        "messages": [{ "role": "user", "content": "hello" }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body = json_body(response).await;
    assert_eq!(body["error"]["type"], "rate_limit_error");
}

async fn capture_non_stream(headers: HeaderMap, Json(body): Json<Value>) -> Response {
    assert_eq!(
        headers.get("x-api-key").unwrap().to_str().unwrap(),
        "test-deepseek-key"
    );
    assert_eq!(body["model"], "deepseek-v4-flash");
    assert_eq!(body["thinking"], json!({ "type": "disabled" }));
    assert!(body.get("output_config").is_none());

    Json(json!({
        "id": "msg_mock",
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": "ok" }],
        "model": "deepseek-v4-flash",
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": { "input_tokens": 3, "output_tokens": 1 }
    }))
    .into_response()
}

async fn capture_round_robin_adapter(headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let key = headers.get("x-api-key").unwrap().to_str().unwrap();
    assert!(matches!(key, "test-deepseek-key" | "second-upstream-key"));
    assert_eq!(body["model"], "deepseek-v4-flash");

    Json(json!({
        "id": "msg_mock",
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": key }],
        "model": "deepseek-v4-flash",
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": { "input_tokens": 3, "output_tokens": 1 }
    }))
    .into_response()
}

async fn capture_sanitized_request(Json(body): Json<Value>) -> Response {
    assert_eq!(body["model"], "deepseek-v4-pro");
    assert_eq!(
        body["messages"][0]["content"][0],
        json!({ "type": "text", "text": "hello" })
    );
    assert_eq!(body["metadata"], json!({ "user_id": "user-1" }));
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
    assert!(body.get("extra_unsupported_field").is_none());

    Json(json!({
        "id": "msg_mock",
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": "ok" }],
        "model": "deepseek-v4-pro",
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": { "input_tokens": 3, "output_tokens": 1 }
    }))
    .into_response()
}

async fn count_tokens(headers: HeaderMap, Json(body): Json<Value>) -> Response {
    assert_eq!(
        headers.get("x-api-key").unwrap().to_str().unwrap(),
        "test-deepseek-key"
    );
    assert_eq!(body["model"], "deepseek-v4-pro");
    assert_eq!(body["thinking"], json!({ "type": "disabled" }));
    Json(json!({ "input_tokens": 42 })).into_response()
}

async fn streaming_response() -> Response {
    let body = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_mock\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"deepseek-v4-flash\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    );

    (
        StatusCode::OK,
        [(CONTENT_TYPE, "text/event-stream"), (ACCEPT, "*/*")],
        body,
    )
        .into_response()
}

async fn unexpected_upstream() -> Response {
    panic!("upstream should not be called for locally rejected requests");
}

async fn rate_limited() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({ "error": { "message": "slow down" } })),
    )
        .into_response()
}

async fn json_body(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

struct TestUpstream {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
}

impl TestUpstream {
    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for TestUpstream {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

async fn spawn_upstream(router: Router) -> TestUpstream {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });

    TestUpstream {
        addr,
        shutdown: Some(shutdown_tx),
    }
}
