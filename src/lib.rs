mod anthropic;
mod config;
mod error;
mod upstream;

pub use config::Config;
pub use error::Error;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use tokio::net::TcpListener;
use tracing::info;

#[derive(Clone)]
pub struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) client: reqwest::Client,
}

pub fn app(config: Config) -> Router {
    let state = AppState {
        config: Arc::new(config),
        client: reqwest::Client::new(),
    };

    Router::new()
        .route("/healthz", get(anthropic::healthz))
        .route("/v1/models", get(anthropic::models))
        .route("/v1/messages", post(anthropic::messages))
        .route("/v1/messages/count_tokens", post(anthropic::count_tokens))
        .with_state(state)
}

pub async fn run() -> Result<(), Error> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "deepseed2claude=info,tower_http=info".into()),
        )
        .init();

    let config = Config::from_env()?;
    let addr: SocketAddr = config.listen_addr()?;
    let listener = TcpListener::bind(addr).await?;
    info!(%addr, upstream = %config.messages_url(), "listening");
    axum::serve(listener, app(config)).await?;
    Ok(())
}
