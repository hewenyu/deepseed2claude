mod admin;
mod anthropic;
mod assets;
mod config;
mod error;
mod store;
mod upstream;

pub use config::Config;
pub use error::Error;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use axum::routing::{get, post, put};
use rand::RngCore;
use store::Store;
use tokio::net::TcpListener;
use tracing::info;

#[derive(Clone)]
pub struct AppState {
    pub(crate) config: Arc<Config>,
    pub(crate) client: reqwest::Client,
    pub(crate) store: Store,
    pub(crate) admin_session_token: Arc<str>,
    pub(crate) dispatch_counter: Arc<AtomicU64>,
}

pub async fn app(config: Config) -> Result<Router, Error> {
    let store = Store::connect(&config).await?;
    Ok(app_with_store(config, store))
}

pub async fn test_app(config: Config) -> Result<Router, Error> {
    let store = Store::memory(&config).await?;
    Ok(app_with_store(config, store))
}

fn app_with_store(config: Config, store: Store) -> Router {
    let state = AppState {
        config: Arc::new(config),
        client: reqwest::Client::new(),
        store,
        admin_session_token: Arc::from(new_session_token()),
        dispatch_counter: Arc::new(AtomicU64::new(0)),
    };

    Router::new()
        .route("/healthz", get(anthropic::healthz))
        .route("/v1/models", get(anthropic::models))
        .route("/v1/messages", post(anthropic::messages))
        .route("/v1/messages/count_tokens", post(anthropic::count_tokens))
        .route("/admin", get(assets::admin_index))
        .route("/admin/", get(assets::admin_index))
        .route("/admin/assets/{path}", get(assets::admin_asset))
        .route("/admin/{*path}", get(assets::admin_spa))
        .route("/api/admin/login", post(admin::login))
        .route("/api/admin/logout", post(admin::logout))
        .route("/api/admin/session", get(admin::session))
        .route("/api/admin/state", get(admin::state))
        .route("/api/admin/adapters", post(admin::create_adapter))
        .route(
            "/api/admin/adapters/{id}",
            put(admin::update_adapter).delete(admin::delete_adapter),
        )
        .route("/api/admin/client-keys", post(admin::create_client_key))
        .route(
            "/api/admin/client-keys/{id}",
            put(admin::update_client_key).delete(admin::delete_client_key),
        )
        .with_state(state)
}

impl AppState {
    pub(crate) fn next_dispatch_slot(&self) -> u64 {
        self.dispatch_counter.fetch_add(1, Ordering::Relaxed)
    }
}

fn new_session_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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
    axum::serve(listener, app(config).await?).await?;
    Ok(())
}
