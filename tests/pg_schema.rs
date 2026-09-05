//! `PgStateStore::initialize_schema` against a real Postgres.
//!
//! The rest of the suite runs on `FakeStateStore`, a `HashMap` with no schema,
//! so none of it reaches the `DO $$` blocks. This does: it builds the schema an
//! older install would have, upgrades it, and checks the result against a fresh
//! install of the same function.

use self_host::bootstrap::PG_IMAGE;
use self_host::db::{ApplicationRecord, PgStateStore, StateStore};
use sqlx::{PgPool, Row};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};

/// The schema as it stood before the surrogate-id migration: `name` is the
/// primary key of `applications` and `application_env` points at it.
const OLD_SCHEMA: &str = r#"
    CREATE TABLE platform_state (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE applications (
        name TEXT PRIMARY KEY,
        hostname TEXT NOT NULL,
        image TEXT NOT NULL,
        status TEXT NOT NULL,
        source TEXT NOT NULL DEFAULT 'image'
    );
    CREATE TABLE application_env (
        app_name TEXT NOT NULL REFERENCES applications(name) ON DELETE CASCADE,
        key TEXT NOT NULL,
        value TEXT NOT NULL,
        PRIMARY KEY (app_name, key)
    );
    CREATE TABLE api_keys (
        id TEXT PRIMARY KEY,
        label TEXT NOT NULL,
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
    );
"#;

/// Starts the same Postgres the Platform runs, and hands back one database per
/// name so a single container can hold both an upgraded and a fresh install.
struct Pg {
    _container: ContainerAsync<Postgres>,
    port: u16,
}

impl Pg {
    async fn start() -> Pg {
        let (_, tag) = PG_IMAGE
            .split_once(':')
            .expect("PG_IMAGE names a tag, not a bare image");

        let container = Postgres::default()
            .with_tag(tag)
            .start()
            .await
            .expect("start Postgres (is the Docker daemon running?)");
        let port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("Postgres publishes 5432");

        Pg {
            _container: container,
            port,
        }
    }

    fn url(&self, database: &str) -> String {
        format!(
            "postgres://postgres:postgres@127.0.0.1:{}/{database}",
            self.port
        )
    }

    /// An empty database of its own. Two installs in one container would
    /// otherwise share `public` and the fresh one would find the old tables.
    async fn database(&self, name: &str) -> PgPool {
        let admin = connect(&self.url("postgres")).await;
        sqlx::query(&format!("CREATE DATABASE {name}"))
            .execute(&admin)
            .await
            .unwrap_or_else(|e| panic!("create database {name}: {e}"));
        connect(&self.url(name)).await
    }
}

/// The container is up before its Postgres accepts connections; retry rather
/// than trade a schema test for a flaky one.
async fn connect(url: &str) -> PgPool {
    let mut last = None;
    for _ in 0..30 {
        match PgPool::connect(url).await {
            Ok(pool) => return pool,
            Err(e) => {
                last = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }
    panic!("connect to {url}: {}", last.expect("a failure to report"));
}

async fn store(url: &str) -> PgStateStore {
    PgStateStore::connect(url)
        .await
        .unwrap_or_else(|e| panic!("PgStateStore::connect: {e}"))
}

/// Columns and constraints of the `public` schema, as comparable lines.
///
/// Sorted by name rather than by position on purpose: an upgraded install
/// appends `id` last while a fresh one declares it first, and that difference
/// is not one anybody has to care about.
async fn shape(pool: &PgPool) -> Vec<String> {
    let mut lines = Vec::new();

    let columns = sqlx::query(
        "SELECT table_name, column_name, data_type, is_nullable, \
                coalesce(column_default, '') AS column_default \
         FROM information_schema.columns \
         WHERE table_schema = 'public' \
         ORDER BY table_name, column_name",
    )
    .fetch_all(pool)
    .await
    .expect("read columns");

    for row in columns {
        lines.push(format!(
            "column {}.{} {} nullable={} default={}",
            row.get::<String, _>("table_name"),
            row.get::<String, _>("column_name"),
            row.get::<String, _>("data_type"),
            row.get::<String, _>("is_nullable"),
            row.get::<String, _>("column_default"),
        ));
    }

    let constraints = sqlx::query(
        "SELECT c.conrelid::regclass::text AS table_name, c.conname, \
                pg_get_constraintdef(c.oid) AS def \
         FROM pg_constraint c \
         JOIN pg_namespace n ON n.oid = c.connamespace \
         WHERE n.nspname = 'public' \
         ORDER BY 1, 2",
    )
    .fetch_all(pool)
    .await
    .expect("read constraints");

    for row in constraints {
        lines.push(format!(
            "constraint {}.{} {}",
            row.get::<String, _>("table_name"),
            row.get::<String, _>("conname"),
            row.get::<String, _>("def"),
        ));
    }

    lines
}

/// A shape mismatch reported as what differs, not as two thirty-line vectors.
fn assert_same_shape(actual: &[String], expected: &[String], context: &str) {
    let unexpected: Vec<&String> = actual.iter().filter(|l| !expected.contains(l)).collect();
    let missing: Vec<&String> = expected.iter().filter(|l| !actual.contains(l)).collect();
    assert!(
        unexpected.is_empty() && missing.is_empty(),
        "{context}\n  unexpected: {unexpected:#?}\n  missing: {missing:#?}"
    );
}

async fn constraint_def(pool: &PgPool, name: &str) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT pg_get_constraintdef(oid) FROM pg_constraint WHERE conname = $1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
    .expect("read constraint")
}

#[tokio::test]
async fn upgrading_from_name_as_primary_key_keeps_ids_and_env() {
    let pg = Pg::start().await;
    let pool = pg.database("upgraded").await;

    sqlx::raw_sql(OLD_SCHEMA)
        .execute(&pool)
        .await
        .expect("build the pre-migration schema");

    sqlx::query(
        "INSERT INTO applications (name, hostname, image, status, source) \
         VALUES ('blog', 'blog.home.lan', 'nginx:latest', 'running', 'image')",
    )
    .execute(&pool)
    .await
    .expect("insert an Application the old way");
    sqlx::query(
        "INSERT INTO application_env (app_name, key, value) \
         VALUES ('blog', 'PORT', '8080'), ('blog', 'LOG_LEVEL', 'debug')",
    )
    .execute(&pool)
    .await
    .expect("insert env the old way");

    let store = store(&pg.url("upgraded")).await;
    store
        .initialize_schema()
        .await
        .expect("migrate the old schema");

    // The point of `id = name`: `sf-app-blog` still names the container that is
    // already running, so upgrading restarts nothing.
    let app = store
        .get_application("blog")
        .await
        .expect("read back by id")
        .expect("the Application survived the migration");
    assert_eq!(app.id, "blog");
    assert_eq!(app.name, "blog");
    assert_eq!(app.hostname, "blog.home.lan");
    assert_eq!(app.source, "image");
    assert!(app.aliases.is_empty());
    assert!(app.last_error.is_none());

    let mut env = store.get_all_env("blog").await.expect("read back env");
    env.sort();
    assert_eq!(
        env,
        vec![
            ("LOG_LEVEL".to_string(), "debug".to_string()),
            ("PORT".to_string(), "8080".to_string()),
        ]
    );

    assert_eq!(
        constraint_def(&pool, "applications_pkey").await.as_deref(),
        Some("PRIMARY KEY (id)"),
    );
    assert_eq!(
        constraint_def(&pool, "applications_name_key")
            .await
            .as_deref(),
        Some("UNIQUE (name)"),
    );
    assert_eq!(
        constraint_def(&pool, "application_env_pkey")
            .await
            .as_deref(),
        Some("PRIMARY KEY (app_id, key)"),
    );
    assert_eq!(
        constraint_def(&pool, "application_env_app_id_fkey")
            .await
            .as_deref(),
        Some("FOREIGN KEY (app_id) REFERENCES applications(id) ON DELETE CASCADE"),
    );
    assert!(
        constraint_def(&pool, "application_env_app_name_fkey")
            .await
            .is_none(),
        "the old foreign key is gone",
    );

    // A definition proves the constraint exists; a delete proves it works.
    store
        .delete_application("blog")
        .await
        .expect("delete the Application");
    assert!(
        store
            .get_all_env("blog")
            .await
            .expect("read env after delete")
            .is_empty(),
        "env rows cascade from the new foreign key",
    );
}

#[tokio::test]
async fn a_second_run_over_a_migrated_schema_changes_nothing() {
    let pg = Pg::start().await;
    let pool = pg.database("twice").await;

    sqlx::raw_sql(OLD_SCHEMA)
        .execute(&pool)
        .await
        .expect("build the pre-migration schema");
    sqlx::query(
        "INSERT INTO applications (name, hostname, image, status, source) \
         VALUES ('blog', 'blog.home.lan', 'nginx:latest', 'running', 'image')",
    )
    .execute(&pool)
    .await
    .expect("insert an Application the old way");
    sqlx::query("INSERT INTO application_env (app_name, key, value) VALUES ('blog', 'PORT', '80')")
        .execute(&pool)
        .await
        .expect("insert env the old way");

    let store = store(&pg.url("twice")).await;
    store.initialize_schema().await.expect("first run");
    let after_first = shape(&pool).await;

    store
        .initialize_schema()
        .await
        .expect("second run over an already migrated schema");
    assert_same_shape(
        &shape(&pool).await,
        &after_first,
        "the second run is not a no-op",
    );

    let app = store
        .get_application("blog")
        .await
        .expect("read back by id")
        .expect("the Application survived both runs");
    assert_eq!(app.id, "blog");
    assert_eq!(
        store.get_all_env("blog").await.expect("read back env"),
        vec![("PORT".to_string(), "80".to_string())],
    );
}

#[tokio::test]
async fn an_upgraded_install_ends_up_shaped_like_a_fresh_one() {
    let pg = Pg::start().await;

    let fresh = pg.database("fresh").await;
    store(&pg.url("fresh"))
        .await
        .initialize_schema()
        .await
        .expect("fresh install");

    let upgraded = pg.database("upgraded").await;
    sqlx::raw_sql(OLD_SCHEMA)
        .execute(&upgraded)
        .await
        .expect("build the pre-migration schema");
    store(&pg.url("upgraded"))
        .await
        .initialize_schema()
        .await
        .expect("upgrade install");

    assert_same_shape(
        &shape(&upgraded).await,
        &shape(&fresh).await,
        "CREATE TABLE IF NOT EXISTS and the migration blocks disagree on the final schema",
    );
}

#[tokio::test]
async fn a_fresh_install_stores_an_application_with_its_env() {
    let pg = Pg::start().await;
    let pool = pg.database("fresh").await;
    let store = store(&pg.url("fresh")).await;
    store.initialize_schema().await.expect("fresh install");

    store
        .insert_application(&ApplicationRecord {
            id: "a1b2c3".into(),
            name: "blog".into(),
            hostname: "blog.home.lan".into(),
            aliases: vec!["www.home.lan".into()],
            image: "nginx:latest".into(),
            status: "running".into(),
            source: "image".into(),
            last_error: None,
            compose: None,
            web_service: None,
            web_port: None,
        })
        .await
        .expect("insert an Application");
    store
        .set_env("a1b2c3", "PORT", "8080")
        .await
        .expect("set env");

    let app = store
        .find_application_by_name("blog")
        .await
        .expect("look up by name")
        .expect("the Application is there");
    assert_eq!(app.id, "a1b2c3");
    assert_eq!(app.aliases, vec!["www.home.lan".to_string()]);

    store.delete_application("a1b2c3").await.expect("delete");
    let orphans = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM application_env")
        .fetch_one(&pool)
        .await
        .expect("count env rows");
    assert_eq!(orphans, 0, "env rows cascade on a fresh install too");
}
