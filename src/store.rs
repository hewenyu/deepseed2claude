use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

use crate::{Config, Error};

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Adapter {
    pub id: i64,
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing)]
    pub base_url_override: Option<String>,
    pub api_key: String,
    pub enabled: bool,
    pub priority: i64,
    pub default_model: String,
    pub opus_model: String,
    pub sonnet_model: String,
    pub haiku_model: String,
    pub thinking: Option<String>,
    pub reasoning_effort: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ClientKey {
    pub id: i64,
    pub name: String,
    pub api_key: String,
    pub enabled: bool,
    pub priority: i64,
}

#[derive(Clone, Debug)]
pub struct UpstreamSelection {
    pub adapter: Adapter,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AdapterInput {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub base_url_override: Option<String>,
    pub api_key: String,
    pub enabled: bool,
    pub priority: i64,
    pub default_model: String,
    pub opus_model: String,
    pub sonnet_model: String,
    pub haiku_model: String,
    pub thinking: Option<String>,
    pub reasoning_effort: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct KeyInput {
    pub name: String,
    pub api_key: String,
    pub enabled: bool,
    pub priority: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct AdminState {
    pub adapters: Vec<Adapter>,
    pub client_keys: Vec<ClientKey>,
}

impl Store {
    pub async fn connect(config: &Config) -> Result<Self, Error> {
        let options = SqliteConnectOptions::from_str(&config.database_url)?
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        let store = Self { pool };
        store.migrate().await?;
        store.seed_from_config(config).await?;
        Ok(store)
    }

    pub async fn memory(config: &Config) -> Result<Self, Error> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;
        let store = Self { pool };
        store.migrate().await?;
        store.seed_from_config(config).await?;
        Ok(store)
    }

    pub async fn admin_state(&self) -> Result<AdminState, Error> {
        Ok(AdminState {
            adapters: self.list_adapters().await?,
            client_keys: self.list_client_keys().await?,
        })
    }

    pub async fn authenticate_client_key(&self, api_key: &str) -> Result<bool, Error> {
        if api_key.trim().is_empty() {
            return Ok(false);
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM client_keys WHERE api_key = ? AND enabled = 1",
        )
        .bind(api_key.trim())
        .fetch_one(&self.pool)
        .await?;
        Ok(count > 0)
    }

    pub async fn select_upstream(&self, slot: u64) -> Result<UpstreamSelection, Error> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, kind, base_url_override, api_key, enabled, priority,
                   default_model, opus_model, sonnet_model, haiku_model, thinking, reasoning_effort
            FROM adapters
            WHERE enabled = 1
            ORDER BY priority ASC, id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Err(Error::Config("no enabled adapter configured".to_owned()));
        };
        let row = &rows[slot as usize % rows.len()];

        Ok(UpstreamSelection {
            adapter: adapter_from_row(row),
        })
    }

    pub async fn create_adapter(&self, input: AdapterInput) -> Result<Adapter, Error> {
        validate_adapter_input(&input)?;
        let id = sqlx::query(
            r#"
            INSERT INTO adapters (
                name, kind, base_url_override, api_key, enabled, priority,
                default_model, opus_model, sonnet_model, haiku_model, thinking, reasoning_effort
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(input.name.trim())
        .bind(input.kind.trim())
        .bind(optional_trim(input.base_url_override))
        .bind(input.api_key.trim())
        .bind(bool_int(input.enabled))
        .bind(input.priority)
        .bind(input.default_model.trim())
        .bind(input.opus_model.trim())
        .bind(input.sonnet_model.trim())
        .bind(input.haiku_model.trim())
        .bind(optional_trim(input.thinking))
        .bind(optional_trim(input.reasoning_effort))
        .execute(&self.pool)
        .await?
        .last_insert_rowid();
        self.get_adapter(id).await
    }

    pub async fn update_adapter(&self, id: i64, input: AdapterInput) -> Result<Adapter, Error> {
        validate_adapter_input(&input)?;
        let result = sqlx::query(
            r#"
            UPDATE adapters
            SET name = ?, kind = ?, base_url_override = ?, api_key = ?, enabled = ?, priority = ?,
                default_model = ?, opus_model = ?, sonnet_model = ?,
                haiku_model = ?, thinking = ?, reasoning_effort = ?
            WHERE id = ?
            "#,
        )
        .bind(input.name.trim())
        .bind(input.kind.trim())
        .bind(optional_trim(input.base_url_override))
        .bind(input.api_key.trim())
        .bind(bool_int(input.enabled))
        .bind(input.priority)
        .bind(input.default_model.trim())
        .bind(input.opus_model.trim())
        .bind(input.sonnet_model.trim())
        .bind(input.haiku_model.trim())
        .bind(optional_trim(input.thinking))
        .bind(optional_trim(input.reasoning_effort))
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(Error::InvalidRequest(format!("adapter {id} does not exist")));
        }
        self.get_adapter(id).await
    }

    pub async fn delete_adapter(&self, id: i64) -> Result<(), Error> {
        let result = sqlx::query("DELETE FROM adapters WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(Error::InvalidRequest(format!("adapter {id} does not exist")));
        }
        Ok(())
    }

    pub async fn create_client_key(&self, input: KeyInput) -> Result<ClientKey, Error> {
        validate_key_input(&input)?;
        let id = sqlx::query(
            "INSERT INTO client_keys (name, api_key, enabled, priority) VALUES (?, ?, ?, ?)",
        )
        .bind(input.name.trim())
        .bind(input.api_key.trim())
        .bind(bool_int(input.enabled))
        .bind(input.priority)
        .execute(&self.pool)
        .await?
        .last_insert_rowid();
        self.get_client_key(id).await
    }

    pub async fn update_client_key(&self, id: i64, input: KeyInput) -> Result<ClientKey, Error> {
        validate_key_input(&input)?;
        let result = sqlx::query(
            "UPDATE client_keys SET name = ?, api_key = ?, enabled = ?, priority = ? WHERE id = ?",
        )
        .bind(input.name.trim())
        .bind(input.api_key.trim())
        .bind(bool_int(input.enabled))
        .bind(input.priority)
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(Error::InvalidRequest(format!("client key {id} does not exist")));
        }
        self.get_client_key(id).await
    }

    pub async fn delete_client_key(&self, id: i64) -> Result<(), Error> {
        let result = sqlx::query("DELETE FROM client_keys WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(Error::InvalidRequest(format!("client key {id} does not exist")));
        }
        Ok(())
    }

    async fn migrate(&self) -> Result<(), Error> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS adapters (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                base_url_override TEXT,
                api_key TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                priority INTEGER NOT NULL DEFAULT 100,
                default_model TEXT NOT NULL,
                opus_model TEXT NOT NULL,
                sonnet_model TEXT NOT NULL,
                haiku_model TEXT NOT NULL,
                thinking TEXT,
                reasoning_effort TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS client_keys (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                api_key TEXT NOT NULL UNIQUE,
                enabled INTEGER NOT NULL DEFAULT 1,
                priority INTEGER NOT NULL DEFAULT 100,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn seed_from_config(&self, config: &Config) -> Result<(), Error> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM adapters")
            .fetch_one(&self.pool)
            .await?;
        if count > 0 {
            return Ok(());
        }

        if !config.deepseek_api_key.trim().is_empty() {
            self.create_adapter(AdapterInput {
                name: "DeepSeek".to_owned(),
                kind: "deepseek".to_owned(),
                base_url_override: config.test_deepseek_base_url.clone(),
                api_key: config.deepseek_api_key.clone(),
                enabled: true,
                priority: 10,
                default_model: config.default_deepseek_model.clone(),
                opus_model: config.claude_opus_model.clone(),
                sonnet_model: config.claude_sonnet_model.clone(),
                haiku_model: config.claude_haiku_model.clone(),
                thinking: config.deepseek_thinking.clone(),
                reasoning_effort: config.deepseek_reasoning_effort.clone(),
            })
            .await?;
        }

        self.create_client_key(KeyInput {
            name: "default claude code".to_owned(),
            api_key: "test".to_owned(),
            enabled: true,
            priority: 10,
        })
        .await?;

        Ok(())
    }

    async fn list_adapters(&self) -> Result<Vec<Adapter>, Error> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, kind, base_url_override, api_key, enabled, priority,
                   default_model, opus_model, sonnet_model, haiku_model, thinking, reasoning_effort
            FROM adapters
            ORDER BY priority ASC, id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(adapter_from_row).collect())
    }

    async fn list_client_keys(&self) -> Result<Vec<ClientKey>, Error> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, api_key, enabled, priority
            FROM client_keys
            ORDER BY priority ASC, id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(client_key_from_row).collect())
    }

    async fn get_adapter(&self, id: i64) -> Result<Adapter, Error> {
        let row = sqlx::query(
            r#"
            SELECT id, name, kind, base_url_override, api_key, enabled, priority,
                   default_model, opus_model, sonnet_model, haiku_model, thinking, reasoning_effort
            FROM adapters
            WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref()
            .map(adapter_from_row)
            .ok_or_else(|| Error::InvalidRequest(format!("adapter {id} does not exist")))
    }

    async fn get_client_key(&self, id: i64) -> Result<ClientKey, Error> {
        let row =
            sqlx::query("SELECT id, name, api_key, enabled, priority FROM client_keys WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        row.as_ref()
            .map(client_key_from_row)
            .ok_or_else(|| Error::InvalidRequest(format!("client key {id} does not exist")))
    }
}

impl Adapter {
    pub const DEEPSEEK_BASE_URL: &'static str = "https://api.deepseek.com/anthropic";
    pub const DEEPSEEK_UPSTREAM_PROTOCOL: &'static str = "anthropic";

    pub fn base_url(&self) -> Result<&'static str, Error> {
        if self.base_url_override.is_some() {
            return Err(Error::Config(
                "base URL override must be read through base_url_string".to_owned(),
            ));
        }
        match self.kind.as_str() {
            "deepseek" => Ok(Self::DEEPSEEK_BASE_URL),
            unsupported => Err(Error::InvalidRequest(format!(
                "unsupported adapter kind: {unsupported}"
            ))),
        }
    }

    pub fn messages_url(&self) -> Result<String, Error> {
        let _protocol = Self::DEEPSEEK_UPSTREAM_PROTOCOL;
        let base_url = if let Some(base_url) = &self.base_url_override {
            base_url.clone()
        } else {
            self.base_url()?.to_owned()
        };
        let base = base_url.trim_end_matches('/');
        if base.ends_with("/v1/messages") {
            Ok(base.to_owned())
        } else {
            Ok(format!("{base}/v1/messages"))
        }
    }

    pub fn count_tokens_url(&self) -> Result<String, Error> {
        Ok(format!("{}/count_tokens", self.messages_url()?))
    }

    pub fn map_model(&self, requested_model: &str) -> String {
        let model = requested_model.to_ascii_lowercase();
        if model == "deepseek-v4-flash" || model == "deepseek-v4-pro" {
            return requested_model.to_owned();
        }
        if model.contains("opus") {
            return self.opus_model.clone();
        }
        if model.contains("sonnet") {
            return self.sonnet_model.clone();
        }
        if model.contains("haiku") {
            return self.haiku_model.clone();
        }
        self.default_model.clone()
    }
}

impl UpstreamSelection {
    pub fn messages_url(&self) -> Result<String, Error> {
        self.adapter.messages_url()
    }

    pub fn count_tokens_url(&self) -> Result<String, Error> {
        self.adapter.count_tokens_url()
    }

    pub fn api_key(&self) -> &str {
        &self.adapter.api_key
    }
}

fn validate_adapter_input(input: &AdapterInput) -> Result<(), Error> {
    for (name, value) in [
        ("name", input.name.as_str()),
        ("kind", input.kind.as_str()),
        ("api_key", input.api_key.as_str()),
        ("default_model", input.default_model.as_str()),
        ("opus_model", input.opus_model.as_str()),
        ("sonnet_model", input.sonnet_model.as_str()),
        ("haiku_model", input.haiku_model.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(Error::InvalidRequest(format!("{name} is required")));
        }
    }
    if input.kind.trim() != "deepseek" {
        return Err(Error::InvalidRequest(
            "only deepseek adapters are currently supported".to_owned(),
        ));
    }
    Ok(())
}

fn validate_key_input(input: &KeyInput) -> Result<(), Error> {
    if input.name.trim().is_empty() {
        return Err(Error::InvalidRequest("name is required".to_owned()));
    }
    if input.api_key.trim().is_empty() {
        return Err(Error::InvalidRequest("api_key is required".to_owned()));
    }
    Ok(())
}

fn adapter_from_row(row: &sqlx::sqlite::SqliteRow) -> Adapter {
    Adapter {
        id: row.get("id"),
        name: row.get("name"),
        kind: row.get("kind"),
        base_url_override: row.get("base_url_override"),
        api_key: row.get("api_key"),
        enabled: int_bool(row.get("enabled")),
        priority: row.get("priority"),
        default_model: row.get("default_model"),
        opus_model: row.get("opus_model"),
        sonnet_model: row.get("sonnet_model"),
        haiku_model: row.get("haiku_model"),
        thinking: row.get("thinking"),
        reasoning_effort: row.get("reasoning_effort"),
    }
}

fn client_key_from_row(row: &sqlx::sqlite::SqliteRow) -> ClientKey {
    ClientKey {
        id: row.get("id"),
        name: row.get("name"),
        api_key: row.get("api_key"),
        enabled: int_bool(row.get("enabled")),
        priority: row.get("priority"),
    }
}

fn optional_trim(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_owned();
        if value.is_empty() { None } else { Some(value) }
    })
}

fn bool_int(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

fn int_bool(value: i64) -> bool {
    value != 0
}
