use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

#[derive(Debug)]
pub enum DbError {
    Connection(String),
    Query(String),
    AlreadyInitialized,
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbError::Connection(msg) => write!(f, "DB connection error: {msg}"),
            DbError::Query(msg) => write!(f, "DB query error: {msg}"),
            DbError::AlreadyInitialized => write!(f, "already initialized"),
        }
    }
}

impl std::error::Error for DbError {}

#[async_trait]
pub trait StateStore: Clone + Send + Sync + 'static {
    async fn initialize_schema(&self) -> Result<(), DbError>;
    async fn is_initialized(&self) -> Result<bool, DbError>;
    async fn store_state(&self, key: &str, value: &str) -> Result<(), DbError>;
    async fn get_state(&self, key: &str) -> Result<Option<String>, DbError>;
    async fn get_api_key(&self) -> Result<Option<String>, DbError> {
        self.get_state("api_key").await
    }
}

#[derive(Clone)]
pub struct PgStateStore {
    pool: sqlx::PgPool,
}

impl PgStateStore {
    pub async fn connect(url: &str) -> Result<Self, DbError> {
        let pool = sqlx::PgPool::connect(url)
            .await
            .map_err(|e| DbError::Connection(e.to_string()))?;
        Ok(PgStateStore { pool })
    }
}

#[async_trait]
impl StateStore for PgStateStore {
    async fn initialize_schema(&self) -> Result<(), DbError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS platform_state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(())
    }

    async fn is_initialized(&self) -> Result<bool, DbError> {
        let dns_suffix = self.get_state("dns_suffix").await?;
        Ok(dns_suffix.is_some())
    }

    async fn store_state(&self, key: &str, value: &str) -> Result<(), DbError> {
        sqlx::query(
            r#"
            INSERT INTO platform_state (key, value)
            VALUES ($1, $2)
            ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value
            "#,
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(())
    }

    async fn get_state(&self, key: &str) -> Result<Option<String>, DbError> {
        let row =
            sqlx::query_scalar::<_, String>("SELECT value FROM platform_state WHERE key = $1")
                .bind(key)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(row)
    }
}

#[derive(Clone)]
pub struct FakeStateStore {
    data: Arc<RwLock<HashMap<String, String>>>,
}

impl FakeStateStore {
    pub fn new() -> Self {
        FakeStateStore {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for FakeStateStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StateStore for FakeStateStore {
    async fn initialize_schema(&self) -> Result<(), DbError> {
        Ok(())
    }

    async fn is_initialized(&self) -> Result<bool, DbError> {
        let data = self.data.read().await;
        Ok(data.contains_key("dns_suffix"))
    }

    async fn store_state(&self, key: &str, value: &str) -> Result<(), DbError> {
        let mut data = self.data.write().await;
        data.insert(key.to_string(), value.to_string());
        Ok(())
    }

    async fn get_state(&self, key: &str) -> Result<Option<String>, DbError> {
        let data = self.data.read().await;
        Ok(data.get(key).cloned())
    }
}
