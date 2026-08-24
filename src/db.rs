use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::error::ErrorReport;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationRecord {
    /// Stable identity. The container and the Traefik router are named from
    /// this, never from `name`, so renaming an Application touches only a row.
    pub id: String,
    pub name: String,
    pub hostname: String,
    /// Extra Hostnames the same Application answers on, so changing the
    /// Hostname does not have to break the old one (ADR-0009).
    pub aliases: Vec<String>,
    pub image: String,
    pub status: String,
    pub source: String,
    /// Why the last deploy failed, when `status` is `failed`. Kept as a
    /// report rather than a sentence so the console can put the failure in the
    /// alert's title and the causes in its body.
    pub last_error: Option<ErrorReport>,
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
    async fn get_application(&self, id: &str) -> Result<Option<ApplicationRecord>, DbError>;
    async fn find_application_by_name(
        &self,
        name: &str,
    ) -> Result<Option<ApplicationRecord>, DbError>;
    async fn application_exists(&self, name: &str) -> Result<bool, DbError> {
        Ok(self.find_application_by_name(name).await?.is_some())
    }
    async fn list_applications(&self) -> Result<Vec<ApplicationRecord>, DbError>;
    async fn delete_application(&self, id: &str) -> Result<(), DbError>;

    async fn set_env(&self, app_id: &str, key: &str, value: &str) -> Result<(), DbError>;
    async fn get_env(&self, app_id: &str, key: &str) -> Result<Option<String>, DbError>;
    async fn get_all_env(&self, app_id: &str) -> Result<Vec<(String, String)>, DbError>;
    async fn unset_env(&self, app_id: &str, key: &str) -> Result<(), DbError>;

    async fn list_api_keys(&self) -> Result<Vec<ApiKeyRecord>, DbError>;
    async fn create_api_key(&self, id: &str, label: &str) -> Result<(), DbError>;
    async fn revoke_api_key(&self, id: &str) -> Result<(), DbError>;
}

#[derive(Debug, Clone)]
pub struct ApiKeyRecord {
    pub id: String,
    pub label: String,
    pub created_at: String,
}

#[derive(sqlx::FromRow)]
struct PgApplicationRow {
    id: String,
    name: String,
    hostname: String,
    aliases: Vec<String>,
    image: String,
    status: String,
    source: String,
    last_error: Option<sqlx::types::Json<ErrorReport>>,
}

impl From<PgApplicationRow> for ApplicationRecord {
    fn from(r: PgApplicationRow) -> Self {
        ApplicationRecord {
            id: r.id,
            name: r.name,
            hostname: r.hostname,
            aliases: r.aliases,
            image: r.image,
            status: r.status,
            source: r.source,
            last_error: r.last_error.map(|j| j.0),
        }
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

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS applications (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                hostname TEXT NOT NULL,
                aliases TEXT[] NOT NULL DEFAULT '{}',
                image TEXT NOT NULL,
                status TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT 'image',
                last_error JSONB
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS application_env (
                app_id TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                PRIMARY KEY (app_id, key)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        // Migration: add source column if upgrading from older schema
        let _ = sqlx::query(
            "ALTER TABLE applications ADD COLUMN IF NOT EXISTS source TEXT NOT NULL DEFAULT 'image'",
        )
        .execute(&self.pool)
        .await;

        // Migration: name used to be the primary key, which made renaming an
        // Application mean recreating its container. Introduce a surrogate id.
        //
        // Existing rows get `id = name` on purpose: container_name_for(id) then
        // resolves to the container that is already running, so upgrading does
        // not restart anything.
        sqlx::query(
            r#"
            DO $$
            BEGIN
                IF EXISTS (
                    SELECT 1 FROM information_schema.key_column_usage
                    WHERE constraint_name = 'applications_pkey'
                      AND table_name = 'applications'
                      AND column_name = 'name'
                ) THEN
                    ALTER TABLE applications ADD COLUMN IF NOT EXISTS id TEXT;
                    UPDATE applications SET id = name WHERE id IS NULL;

                    ALTER TABLE application_env ADD COLUMN IF NOT EXISTS app_id TEXT;
                    UPDATE application_env SET app_id = app_name WHERE app_id IS NULL;

                    ALTER TABLE application_env
                        DROP CONSTRAINT IF EXISTS application_env_app_name_fkey;
                    ALTER TABLE application_env DROP COLUMN IF EXISTS app_name;
                    ALTER TABLE application_env ALTER COLUMN app_id SET NOT NULL;
                    ALTER TABLE application_env ADD PRIMARY KEY (app_id, key);

                    ALTER TABLE applications DROP CONSTRAINT applications_pkey;
                    ALTER TABLE applications ALTER COLUMN id SET NOT NULL;
                    ALTER TABLE applications ADD PRIMARY KEY (id);
                    ALTER TABLE applications ADD CONSTRAINT applications_name_key UNIQUE (name);

                    ALTER TABLE application_env
                        ADD CONSTRAINT application_env_app_id_fkey
                        FOREIGN KEY (app_id) REFERENCES applications(id) ON DELETE CASCADE;
                END IF;
            END $$;
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        // ADR-0009: an Application can answer on more than one Hostname.
        let _ = sqlx::query(
            "ALTER TABLE applications ADD COLUMN IF NOT EXISTS aliases TEXT[] NOT NULL DEFAULT '{}'",
        )
        .execute(&self.pool)
        .await;

        let _ = sqlx::query("ALTER TABLE applications ADD COLUMN IF NOT EXISTS last_error JSONB")
            .execute(&self.pool)
            .await;

        // Migration: last_error used to be one flattened sentence. It is now a
        // failure plus its causes, so an install that already has rows keeps
        // what it recorded — as a failure with nothing underneath it.
        sqlx::query(
            r#"
            DO $$
            BEGIN
                IF EXISTS (
                    SELECT 1 FROM information_schema.columns
                    WHERE table_name = 'applications'
                      AND column_name = 'last_error'
                      AND data_type = 'text'
                ) THEN
                    ALTER TABLE applications
                        ALTER COLUMN last_error TYPE JSONB
                        USING CASE
                            WHEN last_error IS NULL THEN NULL
                            ELSE jsonb_build_object(
                                'error', last_error,
                                'caused_by', '[]'::jsonb
                            )
                        END;
                END IF;
            END $$;
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        // An Application with no failure has no report, not a report with
        // nothing in it. The first cut of the migration above wrote one anyway
        // for every row that had never failed; those rows cannot be read back.
        let _ = sqlx::query(
            "UPDATE applications SET last_error = NULL \
             WHERE last_error IS NOT NULL AND last_error->>'error' IS NULL",
        )
        .execute(&self.pool)
        .await;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS api_keys (
                id TEXT PRIMARY KEY,
                label TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
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
            INSERT INTO applications (id, name, hostname, aliases, image, status, source, last_error)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (id) DO UPDATE SET
                name = EXCLUDED.name,
                hostname = EXCLUDED.hostname,
                aliases = EXCLUDED.aliases,
                image = EXCLUDED.image,
                status = EXCLUDED.status,
                source = EXCLUDED.source,
                last_error = EXCLUDED.last_error
            "#,
        )
        .bind(&app.id)
        .bind(&app.name)
        .bind(&app.hostname)
        .bind(&app.aliases)
        .bind(&app.image)
        .bind(&app.status)
        .bind(&app.source)
        .bind(app.last_error.as_ref().map(sqlx::types::Json))
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
        let rows = sqlx::query_as::<_, PgApplicationRow>(
            "SELECT id, name, hostname, aliases, image, status, source, last_error \
             FROM applications ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(rows.into_iter().map(ApplicationRecord::from).collect())
    }

    async fn get_application(&self, id: &str) -> Result<Option<ApplicationRecord>, DbError> {
        let row = sqlx::query_as::<_, PgApplicationRow>(
            "SELECT id, name, hostname, aliases, image, status, source, last_error \
             FROM applications WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(row.map(ApplicationRecord::from))
    }

    async fn find_application_by_name(
        &self,
        name: &str,
    ) -> Result<Option<ApplicationRecord>, DbError> {
        let row = sqlx::query_as::<_, PgApplicationRow>(
            "SELECT id, name, hostname, aliases, image, status, source, last_error \
             FROM applications WHERE name = $1",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(row.map(ApplicationRecord::from))
    }

    async fn delete_application(&self, id: &str) -> Result<(), DbError> {
        let result = sqlx::query("DELETE FROM applications WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound(id.to_string()));
        }

        Ok(())
    }

    async fn set_env(&self, app_id: &str, key: &str, value: &str) -> Result<(), DbError> {
        sqlx::query(
            r#"
            INSERT INTO application_env (app_id, key, value)
            VALUES ($1, $2, $3)
            ON CONFLICT (app_id, key) DO UPDATE SET value = EXCLUDED.value
            "#,
        )
        .bind(app_id)
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }

    async fn get_env(&self, app_id: &str, key: &str) -> Result<Option<String>, DbError> {
        let row = sqlx::query_scalar::<_, String>(
            "SELECT value FROM application_env WHERE app_id = $1 AND key = $2",
        )
        .bind(app_id)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(row)
    }

    async fn get_all_env(&self, app_id: &str) -> Result<Vec<(String, String)>, DbError> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT key, value FROM application_env WHERE app_id = $1 ORDER BY key",
        )
        .bind(app_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(rows)
    }

    async fn unset_env(&self, app_id: &str, key: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM application_env WHERE app_id = $1 AND key = $2")
            .bind(app_id)
            .bind(key)
            .execute(&self.pool)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }

    async fn list_api_keys(&self) -> Result<Vec<ApiKeyRecord>, DbError> {
        let rows = sqlx::query_as::<_, (String, String, String)>(
            "SELECT id, label, created_at::TEXT FROM api_keys ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|(id, label, created_at)| ApiKeyRecord {
                id,
                label,
                created_at,
            })
            .collect())
    }

    async fn create_api_key(&self, id: &str, label: &str) -> Result<(), DbError> {
        sqlx::query("INSERT INTO api_keys (id, label) VALUES ($1, $2)")
            .bind(id)
            .bind(label)
            .execute(&self.pool)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;
        Ok(())
    }

    async fn revoke_api_key(&self, id: &str) -> Result<(), DbError> {
        let result = sqlx::query("DELETE FROM api_keys WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound(id.to_string()));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct FakeStateStore {
    data: Arc<RwLock<HashMap<String, String>>>,
    apps: Arc<RwLock<HashMap<String, ApplicationRecord>>>,
    env: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
    api_keys: Arc<RwLock<Vec<ApiKeyRecord>>>,
}

impl FakeStateStore {
    pub fn new() -> Self {
        FakeStateStore {
            data: Arc::new(RwLock::new(HashMap::new())),
            apps: Arc::new(RwLock::new(HashMap::new())),
            env: Arc::new(RwLock::new(HashMap::new())),
            api_keys: Arc::new(RwLock::new(Vec::new())),
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
        apps.insert(app.id.clone(), app.clone());
        Ok(())
    }

    async fn get_application(&self, id: &str) -> Result<Option<ApplicationRecord>, DbError> {
        Ok(self.apps.read().await.get(id).cloned())
    }

    async fn find_application_by_name(
        &self,
        name: &str,
    ) -> Result<Option<ApplicationRecord>, DbError> {
        Ok(self
            .apps
            .read()
            .await
            .values()
            .find(|a| a.name == name)
            .cloned())
    }

    async fn list_applications(&self) -> Result<Vec<ApplicationRecord>, DbError> {
        let apps = self.apps.read().await;
        let mut list: Vec<_> = apps.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn delete_application(&self, id: &str) -> Result<(), DbError> {
        let mut apps = self.apps.write().await;
        if apps.remove(id).is_none() {
            return Err(DbError::NotFound(id.to_string()));
        }
        let mut env = self.env.write().await;
        env.remove(id);
        Ok(())
    }

    async fn set_env(&self, app_id: &str, key: &str, value: &str) -> Result<(), DbError> {
        let mut env = self.env.write().await;
        env.entry(app_id.to_string())
            .or_default()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    async fn get_env(&self, app_id: &str, key: &str) -> Result<Option<String>, DbError> {
        let env = self.env.read().await;
        Ok(env.get(app_id).and_then(|m| m.get(key)).cloned())
    }

    async fn get_all_env(&self, app_id: &str) -> Result<Vec<(String, String)>, DbError> {
        let env = self.env.read().await;
        let mut vars: Vec<_> = env
            .get(app_id)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        vars.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(vars)
    }

    async fn unset_env(&self, app_id: &str, key: &str) -> Result<(), DbError> {
        let mut env = self.env.write().await;
        if let Some(m) = env.get_mut(app_id) {
            m.remove(key);
        }
        Ok(())
    }

    async fn list_api_keys(&self) -> Result<Vec<ApiKeyRecord>, DbError> {
        Ok(self.api_keys.read().await.clone())
    }

    async fn create_api_key(&self, id: &str, label: &str) -> Result<(), DbError> {
        self.api_keys.write().await.push(ApiKeyRecord {
            id: id.to_string(),
            label: label.to_string(),
            created_at: "now".into(),
        });
        Ok(())
    }

    async fn revoke_api_key(&self, id: &str) -> Result<(), DbError> {
        let mut keys = self.api_keys.write().await;
        let len = keys.len();
        keys.retain(|k| k.id != id);
        if keys.len() == len {
            return Err(DbError::NotFound(id.to_string()));
        }
        Ok(())
    }
}
