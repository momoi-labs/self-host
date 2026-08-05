use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationRecord {
    pub name: String,
    pub hostname: String,
    pub image: String,
    pub status: String,
    pub source: String,
}

#[derive(Debug)]
pub enum DbError {
    Connection(String),
    Query(String),
    AlreadyInitialized,
    AlreadyExists(String),
    NotFound(String),
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbError::Connection(msg) => write!(f, "DB connection error: {msg}"),
            DbError::Query(msg) => write!(f, "DB query error: {msg}"),
            DbError::AlreadyInitialized => write!(f, "already initialized"),
            DbError::AlreadyExists(name) => write!(f, "Application '{name}' already exists"),
            DbError::NotFound(name) => write!(f, "Application '{name}' not found"),
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

    async fn insert_application(&self, app: &ApplicationRecord) -> Result<(), DbError>;
    async fn application_exists(&self, name: &str) -> Result<bool, DbError>;
    async fn list_applications(&self) -> Result<Vec<ApplicationRecord>, DbError>;
    async fn delete_application(&self, name: &str) -> Result<(), DbError>;

    async fn set_env(&self, app_name: &str, key: &str, value: &str) -> Result<(), DbError>;
    async fn get_env(&self, app_name: &str, key: &str) -> Result<Option<String>, DbError>;
    async fn get_all_env(&self, app_name: &str) -> Result<Vec<(String, String)>, DbError>;
    async fn unset_env(&self, app_name: &str, key: &str) -> Result<(), DbError>;
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

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS applications (
                name TEXT PRIMARY KEY,
                hostname TEXT NOT NULL,
                image TEXT NOT NULL,
                status TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT 'image'
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS application_env (
                app_name TEXT NOT NULL REFERENCES applications(name) ON DELETE CASCADE,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                PRIMARY KEY (app_name, key)
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

    async fn insert_application(&self, app: &ApplicationRecord) -> Result<(), DbError> {
        sqlx::query(
            r#"
            INSERT INTO applications (name, hostname, image, status, source)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (name) DO UPDATE SET
                hostname = EXCLUDED.hostname,
                image = EXCLUDED.image,
                status = EXCLUDED.status,
                source = EXCLUDED.source
            "#,
        )
        .bind(&app.name)
        .bind(&app.hostname)
        .bind(&app.image)
        .bind(&app.status)
        .bind(&app.source)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(())
    }

    async fn application_exists(&self, name: &str) -> Result<bool, DbError> {
        let row = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM applications WHERE name = $1")
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(row > 0)
    }

    async fn list_applications(&self) -> Result<Vec<ApplicationRecord>, DbError> {
        let rows = sqlx::query_as::<_, (String, String, String, String, String)>(
            "SELECT name, hostname, image, status, source FROM applications ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|(name, hostname, image, status, source)| ApplicationRecord {
                name,
                hostname,
                image,
                status,
                source,
            })
            .collect())
    }

    async fn delete_application(&self, name: &str) -> Result<(), DbError> {
        let result = sqlx::query("DELETE FROM applications WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound(name.to_string()));
        }

        Ok(())
    }

    async fn set_env(&self, app_name: &str, key: &str, value: &str) -> Result<(), DbError> {
        sqlx::query(
            r#"
            INSERT INTO application_env (app_name, key, value)
            VALUES ($1, $2, $3)
            ON CONFLICT (app_name, key) DO UPDATE SET value = EXCLUDED.value
            "#,
        )
        .bind(app_name)
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }

    async fn get_env(&self, app_name: &str, key: &str) -> Result<Option<String>, DbError> {
        let row = sqlx::query_scalar::<_, String>(
            "SELECT value FROM application_env WHERE app_name = $1 AND key = $2",
        )
        .bind(app_name)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(row)
    }

    async fn get_all_env(&self, app_name: &str) -> Result<Vec<(String, String)>, DbError> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT key, value FROM application_env WHERE app_name = $1 ORDER BY key",
        )
        .bind(app_name)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(rows)
    }

    async fn unset_env(&self, app_name: &str, key: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM application_env WHERE app_name = $1 AND key = $2")
            .bind(app_name)
            .bind(key)
            .execute(&self.pool)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct FakeStateStore {
    data: Arc<RwLock<HashMap<String, String>>>,
    apps: Arc<RwLock<HashMap<String, ApplicationRecord>>>,
    env: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
}

impl FakeStateStore {
    pub fn new() -> Self {
        FakeStateStore {
            data: Arc::new(RwLock::new(HashMap::new())),
            apps: Arc::new(RwLock::new(HashMap::new())),
            env: Arc::new(RwLock::new(HashMap::new())),
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

    async fn insert_application(&self, app: &ApplicationRecord) -> Result<(), DbError> {
        let mut apps = self.apps.write().await;
        apps.insert(app.name.clone(), app.clone());
        Ok(())
    }

    async fn application_exists(&self, name: &str) -> Result<bool, DbError> {
        let apps = self.apps.read().await;
        Ok(apps.contains_key(name))
    }

    async fn list_applications(&self) -> Result<Vec<ApplicationRecord>, DbError> {
        let apps = self.apps.read().await;
        let mut list: Vec<_> = apps.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn delete_application(&self, name: &str) -> Result<(), DbError> {
        let mut apps = self.apps.write().await;
        if apps.remove(name).is_none() {
            return Err(DbError::NotFound(name.to_string()));
        }
        let mut env = self.env.write().await;
        env.remove(name);
        Ok(())
    }

    async fn set_env(&self, app_name: &str, key: &str, value: &str) -> Result<(), DbError> {
        let mut env = self.env.write().await;
        env.entry(app_name.to_string())
            .or_default()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    async fn get_env(&self, app_name: &str, key: &str) -> Result<Option<String>, DbError> {
        let env = self.env.read().await;
        Ok(env
            .get(app_name)
            .and_then(|m| m.get(key))
            .cloned())
    }

    async fn get_all_env(&self, app_name: &str) -> Result<Vec<(String, String)>, DbError> {
        let env = self.env.read().await;
        let mut vars: Vec<_> = env
            .get(app_name)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        vars.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(vars)
    }

    async fn unset_env(&self, app_name: &str, key: &str) -> Result<(), DbError> {
        let mut env = self.env.write().await;
        if let Some(m) = env.get_mut(app_name) {
            m.remove(key);
        }
        Ok(())
    }
}
