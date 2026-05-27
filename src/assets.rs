use axum::body::Body;
use axum::extract::Path;
use axum::http::header::CONTENT_TYPE;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "admin-ui/dist"]
struct AdminAssets;

pub async fn admin_index() -> Response {
    serve_asset("index.html")
}

pub async fn admin_asset(Path(path): Path<String>) -> Response {
    serve_asset(&format!("assets/{path}"))
}

pub async fn admin_spa(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches("/admin/");
    if !path.is_empty() && path.contains('.') {
        serve_asset(path)
    } else {
        serve_asset("index.html")
    }
}

fn serve_asset(path: &str) -> Response {
    let Some(asset) = AdminAssets::get(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Response::builder()
        .header(CONTENT_TYPE, mime(path))
        .body(Body::from(asset.data.into_owned()))
        .expect("valid static asset response")
}

fn mime(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    }
}
